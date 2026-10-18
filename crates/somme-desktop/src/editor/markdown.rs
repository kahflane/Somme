//! The Markdown preview, which is also an editor: clicking a block turns it
//! back into its Markdown, and every change goes straight into the file.
//! Mermaid diagrams are drawn, and images load from the web, from this
//! computer, or from the server the file is on.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::base::TextView;
use gpui_kit::component::input::{EditorState, Escape, InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Theme, h_flex, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, Image, ImageFormat, ImageSource, Pixels,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString, SharedUri,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, img, prelude::FluentBuilder as _, px,
};
use markdown::mdast::Node;
use parking_lot::Mutex;

use crate::settings::Settings;
use crate::terminal::disk::Disk;

/// The most read of one image from a server.
const IMAGE_LIMIT: u64 = 20 * 1024 * 1024;

/// One top-level piece of the document: a paragraph, a list, a table, a
/// code block, and so on.
#[derive(Clone, Debug, PartialEq)]
struct Block {
    /// Where it is in the file.
    range: Range<usize>,
    kind: Kind,
}

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    /// Shown as Markdown: this text, which may differ from the source, as
    /// for front matter.
    Markdown(SharedString),
    Mermaid(SharedString),
    /// Shown as its source, as it has no look of its own, like a link
    /// definition.
    Source(SharedString),
}

/// The document's blocks, and its link definitions, which each block needs
/// to show links that use them.
fn parse(text: &str) -> (Vec<Block>, String, Vec<String>) {
    let mut options = markdown::ParseOptions::gfm();
    options.constructs.frontmatter = true;
    let Ok(Node::Root(root)) = markdown::to_mdast(text, &options) else {
        let whole = Block {
            range: 0..text.len(),
            kind: Kind::Source(text.to_owned().into()),
        };
        return (vec![whole], String::new(), Vec::new());
    };
    let mut definitions = String::new();
    let mut images = Vec::new();
    let blocks = root
        .children
        .iter()
        .filter_map(|node| {
            let position = node.position()?;
            let range = position.start.offset..position.end.offset;
            let source = text.get(range.clone())?;
            collect_images(node, &mut images);
            let kind = match node {
                Node::Code(code) if code.lang.as_deref().is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid")) => {
                    Kind::Mermaid(code.value.clone().into())
                }
                Node::Yaml(yaml) => Kind::Markdown(format!("```yaml\n{}\n```", yaml.value).into()),
                Node::Toml(toml) => Kind::Markdown(format!("```toml\n{}\n```", toml.value).into()),
                Node::Definition(_) => {
                    definitions.push_str(source);
                    definitions.push('\n');
                    Kind::Source(source.to_owned().into())
                }
                _ => Kind::Markdown(source.to_owned().into()),
            };
            Some(Block { range, kind })
        })
        .collect();
    (blocks, definitions, images)
}

fn collect_images(node: &Node, images: &mut Vec<String>) {
    if let Node::Image(image) = node {
        images.push(image.url.clone());
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_images(child, images);
        }
    }
}

/// Where an image in the document is: on the web or inline, or a file.
enum Place {
    Web,
    Inline,
    File(PathBuf),
    /// Never loaded: it would reach another computer.
    Blocked,
}

/// Whether a path reaches another computer, as `\\server\share` or
/// `//server/share` do on Windows. Opening one there sends the user's login
/// to that server, so a document from anywhere could collect it.
fn is_network(path: &str) -> bool {
    let mut chars = path.chars();
    matches!((chars.next(), chars.next()), (Some('/' | '\\'), Some('/' | '\\')))
}

fn place(url: &str, folder: Option<&Path>) -> Place {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Place::Web;
    }
    if lower.starts_with("data:") {
        return Place::Inline;
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    // Other kinds of links, like smb://, are not images here.
    if path.contains("://") {
        return Place::Blocked;
    }
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy().into_owned();
    if is_network(&path) {
        return Place::Blocked;
    }
    let path = PathBuf::from(path);
    match folder {
        Some(folder) if path.is_relative() => Place::File(folder.join(path)),
        _ => Place::File(path),
    }
}

/// What kind of image `bytes` are, by their first bytes, or by the name.
fn image_format(bytes: &[u8], name: &str) -> Option<ImageFormat> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    Some(if starts(b"\x89PNG") {
        ImageFormat::Png
    } else if starts(&[0xff, 0xd8]) {
        ImageFormat::Jpeg
    } else if starts(b"GIF8") {
        ImageFormat::Gif
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        ImageFormat::Webp
    } else if starts(b"BM") {
        ImageFormat::Bmp
    } else if name.to_ascii_lowercase().ends_with(".svg") || String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).contains("<svg") {
        ImageFormat::Svg
    } else {
        return None;
    })
}

/// A `data:` URL's image.
fn inline_image(url: &str) -> Option<Image> {
    let (head, data) = url.split_once(',')?;
    let bytes = if head.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(data.trim()).ok()?
    } else {
        percent_encoding::percent_decode_str(data).collect()
    };
    let format = image_format(&bytes, if head.contains("svg") { "x.svg" } else { "" })?;
    Some(Image::from_bytes(format, bytes))
}

enum Diagram {
    Drawing,
    Drawn { image: Arc<Image>, width: f32, height: f32 },
    Failed(String),
}

/// Draws a Mermaid diagram to SVG. Slow for big ones: run it off the UI thread.
fn draw(code: &str, dark: bool) -> Result<(String, f32, f32), String> {
    let options = mermaid_rs_renderer::RenderOptions {
        theme: if dark { mermaid_rs_renderer::Theme::dark() } else { mermaid_rs_renderer::Theme::modern() },
        ..Default::default()
    };
    // A diagram it does not understand must not take the app down.
    let svg = std::panic::catch_unwind(|| mermaid_rs_renderer::render_with_options(code, options))
        .map_err(|_| "The diagram could not be drawn.".to_owned())?
        .map_err(|err| err.to_string())?;
    let (width, height) = svg_size(&svg).ok_or("The diagram has no size.")?;
    Ok((svg, width, height))
}

/// The size an SVG asks for, from its width and height, or its view box.
fn svg_size(svg: &str) -> Option<(f32, f32)> {
    let tag = &svg[svg.find("<svg")?..];
    let tag = &tag[..tag.find('>')?];
    let attribute = |name: &str| {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let value = &tag[at..];
        Some(&value[..value.find('"')?])
    };
    let number = |text: &str| text.trim().trim_end_matches("px").parse::<f32>().ok();
    if let (Some(width), Some(height)) = (attribute("width").and_then(number), attribute("height").and_then(number)) {
        return Some((width, height));
    }
    let view: Vec<f32> = attribute("viewBox")?.split([' ', ',']).filter_map(number).collect();
    (view.len() == 4).then(|| (view[2], view[3]))
}

struct Editing {
    /// Where the block being edited is in the file, as it changes.
    range: Range<usize>,
    input: Entity<TextareaState>,
    _subscription: Subscription,
}

pub struct MarkdownPreview {
    source: Entity<EditorState>,
    disk: Disk,
    /// The file's folder, for images given by a relative path.
    folder: Option<PathBuf>,
    blocks: Vec<Block>,
    definitions: SharedString,
    editing: Option<Editing>,
    diagrams: HashMap<(SharedString, bool), Diagram>,
    /// Images read from the server the file is on, by their path there.
    server_images: Arc<Mutex<HashMap<PathBuf, Arc<Image>>>>,
    asked_images: HashSet<PathBuf>,
    /// Inline images, by their URL.
    inline_images: Arc<Mutex<HashMap<String, Option<Arc<Image>>>>>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
    _watch: Subscription,
}

impl MarkdownPreview {
    pub fn new(source: Entity<EditorState>, path: &Path, disk: Disk, cx: &mut Context<Self>) -> Self {
        let watch = cx.observe(&source, |preview, _, cx| preview.reparse(cx));
        let mut preview = Self {
            source,
            disk,
            folder: path.parent().map(Path::to_owned),
            blocks: Vec::new(),
            definitions: SharedString::default(),
            editing: None,
            diagrams: HashMap::new(),
            server_images: Arc::default(),
            asked_images: HashSet::new(),
            inline_images: Arc::default(),
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
            _watch: watch,
        };
        preview.reparse(cx);
        preview
    }

    fn reparse(&mut self, cx: &mut Context<Self>) {
        let text = self.source.read(cx).text().to_string();
        let (blocks, definitions, images) = parse(&text);
        if blocks == self.blocks && definitions == self.definitions.as_ref() {
            return;
        }
        self.blocks = blocks;
        self.definitions = definitions.into();
        self.load_server_images(images, cx);
        cx.notify();
    }

    /// Reads the images of a file on a server from there.
    fn load_server_images(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        if self.disk.is_local() {
            return;
        }
        for url in urls {
            let Place::File(path) = place(&url, self.folder.as_deref()) else { continue };
            if !self.asked_images.insert(path.clone()) {
                continue;
            }
            let images = self.server_images.clone();
            let read = self.disk.clone().run({
                let path = path.clone();
                move |disk| disk.read(&path, IMAGE_LIMIT)
            });
            cx.spawn(async move |preview, cx| {
                let Ok(bytes) = read.await else { return };
                let name = path.to_string_lossy().into_owned();
                let Some(format) = image_format(&bytes, &name) else { return };
                images.lock().insert(path, Arc::new(Image::from_bytes(format, bytes)));
                let _ = preview.update(cx, |_, cx| cx.notify());
            })
            .detach();
        }
    }

    /// Turns block `ix` back into Markdown to edit.
    fn edit(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(block) = self.blocks.get(ix) else { return };
        let range = block.range.clone();
        let text = self.source.read(cx).text().slice(range.clone()).to_string();
        self.start_editing(range, text, window, cx);
    }

    /// Starts a new block at the end of the document.
    fn append(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.source.read(cx).text().to_string();
        let gap = if text.trim().is_empty() || text.ends_with("\n\n") {
            ""
        } else if text.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let end = text.len();
        self.write(end..end, gap, window, cx);
        self.start_editing(end + gap.len()..end + gap.len(), String::new(), window, cx);
    }

    fn start_editing(&mut self, range: Range<usize>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 400).default_value(text));
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe_in(&input, window, |preview, input, event, window, cx| match event {
            InputEvent::Change => {
                let Some(editing) = preview.editing.as_ref() else { return };
                let range = editing.range.clone();
                let text = input.read(cx).value().to_string();
                preview.write(range.clone(), &text, window, cx);
                if let Some(editing) = preview.editing.as_mut() {
                    editing.range = range.start..range.start + text.len();
                }
            }
            InputEvent::Blur => preview.stop_editing(cx),
            _ => {}
        });
        self.editing = Some(Editing {
            range,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn stop_editing(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            cx.notify();
        }
    }

    /// Puts `text` in place of `range` in the file, as typing there would.
    fn write(&mut self, range: Range<usize>, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.source.update(cx, |source, cx| {
            let end = range.end.min(source.text().len());
            source.set_selected_range(range.start.min(end)..end, cx);
            source.replace(text.to_owned(), window, cx);
        });
    }

    fn image_resolver(&self) -> impl Fn(&SharedUri) -> ImageSource + Send + Sync + 'static {
        let folder = self.folder.clone();
        let local = self.disk.is_local();
        let server_images = self.server_images.clone();
        let inline_images = self.inline_images.clone();
        move |uri: &SharedUri| match place(uri.as_ref(), folder.as_deref()) {
            Place::Web => ImageSource::from(uri.clone()),
            Place::Inline => {
                let image = inline_images
                    .lock()
                    .entry(uri.to_string())
                    .or_insert_with(|| inline_image(uri.as_ref()).map(Arc::new))
                    .clone();
                image.map_or_else(|| ImageSource::from(PathBuf::new()), ImageSource::from)
            }
            Place::File(path) if local => ImageSource::from(path),
            Place::File(path) => {
                server_images.lock().get(&path).cloned().map_or_else(|| ImageSource::from(PathBuf::new()), ImageSource::from)
            }
            Place::Blocked => ImageSource::from(PathBuf::new()),
        }
    }

    fn diagram(&mut self, code: &SharedString, dark: bool, cx: &mut Context<Self>) -> &Diagram {
        let key = (code.clone(), dark);
        if !self.diagrams.contains_key(&key) {
            self.diagrams.insert(key.clone(), Diagram::Drawing);
            let source = code.to_string();
            cx.spawn(async move |preview, cx| {
                let drawn = cx.background_spawn(async move { draw(&source, dark) }).await;
                let _ = preview.update(cx, |preview, cx| {
                    let diagram = match drawn {
                        Ok((svg, width, height)) => Diagram::Drawn {
                            image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
                            width,
                            height,
                        },
                        Err(err) => Diagram::Failed(err),
                    };
                    preview.diagrams.insert(key, diagram);
                    cx.notify();
                });
            })
            .detach();
        }
        &self.diagrams[&(code.clone(), dark)]
    }

    fn render_block(&mut self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme().clone();
        let block = self.blocks[ix].clone();
        let mono = Settings::get(cx).font_family.clone();
        let body = match &block.kind {
            Kind::Markdown(text) => {
                let text = if self.definitions.is_empty() {
                    text.clone()
                } else {
                    format!("{text}\n\n{}", self.definitions).into()
                };
                TextView::markdown(("md-text", ix), text)
                    .selectable(false)
                    .image_source(self.image_resolver())
                    .into_any_element()
            }
            Kind::Source(text) => div()
                .text_xs()
                .font_family(mono)
                .text_color(theme.muted_foreground)
                .child(text.clone())
                .into_any_element(),
            Kind::Mermaid(code) => match self.diagram(code, theme.is_dark(), cx) {
                Diagram::Drawing => div()
                    .py_4()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Drawing the diagram…")
                    .into_any_element(),
                Diagram::Drawn { image, width, height } => h_flex()
                    .w_full()
                    .justify_center()
                    .child(img(image.clone()).w_full().max_w(px(*width)).aspect_ratio(width / height))
                    .into_any_element(),
                Diagram::Failed(err) => v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.danger)
                            .child(SharedString::from(format!("Mermaid: {err}"))),
                    )
                    .child(
                        TextView::markdown(("md-text", ix), format!("```mermaid\n{code}\n```"))
                            .selectable(false),
                    )
                    .into_any_element(),
            },
        };
        div()
            .id(("md-block", ix))
            .w_full()
            .px_3()
            .py_1()
            .rounded_md()
            .cursor_text()
            .hover(|style| style.bg(theme.foreground.opacity(0.03)))
            .on_click(cx.listener(move |preview, _, window, cx| preview.edit(ix, window, cx)))
            .child(body)
    }
}

impl Focusable for MarkdownPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let settings = Settings::get(cx);
        let mono = settings.font_family.clone();
        let size = px(settings.font_size.clamp(6., 72.));
        let editing = self.editing.as_ref().map(|editing| (editing.range.clone(), editing.input.clone()));
        let mut children = Vec::new();
        let mut editor_shown = false;
        for ix in 0..self.blocks.len() {
            let range = self.blocks[ix].range.clone();
            if let Some((edit, input)) = &editing {
                // Blocks the edit touches show as the text being edited.
                if range.start < edit.end && edit.start < range.end {
                    continue;
                }
                if range.start >= edit.end && !editor_shown {
                    children.push(editor(input, &theme, mono.clone(), size).into_any_element());
                    editor_shown = true;
                }
            }
            children.push(self.render_block(ix, cx).into_any_element());
        }
        if let Some((_, input)) = &editing
            && !editor_shown
        {
            children.push(editor(input, &theme, mono, size).into_any_element());
        }
        let empty = self.blocks.is_empty() && editing.is_none();
        div()
            .id("md-preview")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(theme.background)
            .capture_action(cx.listener(|preview, _: &Escape, _, cx| preview.stop_editing(cx)))
            .child(
                v_flex()
                    .w_full()
                    .max_w(px(880.))
                    .mx_auto()
                    .px_6()
                    .py_5()
                    .gap_1()
                    .children(children)
                    .child(
                        div()
                            .id("md-end")
                            .w_full()
                            .min_h(px(160.))
                            .px_3()
                            .cursor_text()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .when(empty, |end| end.child("Click to start writing."))
                            .on_click(cx.listener(|preview, _, window, cx| preview.append(window, cx))),
                    ),
            )
    }
}

fn editor(input: &Entity<TextareaState>, theme: &Theme, mono: String, size: Pixels) -> impl IntoElement {
    div()
        .w_full()
        .px_2()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(theme.ring)
        .bg(theme.foreground.opacity(0.02))
        .child(Textarea::new(input).appearance(false).bordered(false).font_family(mono).text_size(size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_into_blocks_that_cover_their_source() {
        let text = "---\ntitle: x\n---\n# Title\n\nSome [text][a].\n\n```mermaid\ngraph TD; A-->B\n```\n\n- one\n- two\n\n[a]: https://example.test\n";
        let (blocks, definitions, _) = parse(text);
        assert_eq!(blocks.len(), 6, "{blocks:?}");
        assert!(matches!(&blocks[0].kind, Kind::Markdown(shown) if shown.starts_with("```yaml")));
        assert_eq!(&text[blocks[1].range.clone()], "# Title");
        assert!(matches!(&blocks[3].kind, Kind::Mermaid(code) if code.as_ref() == "graph TD; A-->B"));
        assert_eq!(&text[blocks[4].range.clone()], "- one\n- two\n");
        assert_eq!(definitions, "[a]: https://example.test\n");
    }

    #[test]
    fn finds_images_and_where_they_are() {
        let (_, _, images) = parse("![logo](img/logo.png) and ![web](https://example.test/a.png)\n");
        assert_eq!(images, ["img/logo.png", "https://example.test/a.png"]);
        assert!(matches!(place("img/a%20b.png", Some(Path::new("/docs"))), Place::File(path) if path == Path::new("/docs/img/a b.png")));
        assert!(matches!(place("HTTPS://example.test/a.png", None), Place::Web));
        assert!(matches!(place("data:image/png;base64,AAAA", None), Place::Inline));
        for network in [r"\\server\share\a.png", "//server/share/a.png", "file:////server/a.png", "%5C%5Cserver%5Ca.png", r"\\?\UNC\server\a.png", "smb://server/a.png"] {
            assert!(matches!(place(network, Some(Path::new("/docs"))), Place::Blocked), "{network}");
        }
    }

    #[test]
    fn draws_mermaid() {
        let (svg, width, height) = draw("flowchart LR; A-->B-->C", false).unwrap();
        assert!(svg.contains("<svg"));
        assert!(width > 0. && height > 0.);
        assert!(draw("this is not a diagram at all {{{", true).is_err());
    }

    #[test]
    fn knows_image_kinds() {
        assert!(matches!(image_format(b"\x89PNG\r\n", ""), Some(ImageFormat::Png)));
        assert!(matches!(image_format(b"<?xml?><svg ", ""), Some(ImageFormat::Svg)));
        assert!(image_format(b"hello", "a.txt").is_none());
        assert!(inline_image("data:image/svg+xml,%3Csvg%3E%3C/svg%3E").is_some());
    }
}
