//! A GitHub user's profile as github.com shows it: their picture and details
//! beside the README of their repository named like them.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::base::TextView;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Global, Hsla, ImageSource, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, SharedUri, StatefulInteractiveElement as _, Styled as _, Window, div,
    img, prelude::FluentBuilder as _, px, rgb,
};
use markdown::mdast::Node;
use zeroize::Zeroizing;

use super::api::{ApiError, GitHub, Profile};
use crate::runtime;
use crate::services::Services;
use crate::ui::common::Lucide;

/// Below this width the details go above the README, as on a phone.
const NARROW: f32 = 760.;
const AVATAR: f32 = 232.;
/// How long a profile is shown again without asking GitHub, which allows
/// only so many asks an hour.
const KEEP: Duration = Duration::from_secs(10 * 60);

/// github.com's own colours.
struct Palette {
    background: Hsla,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    button: Hsla,
}

fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            background: rgb(0x0d1117).into(),
            foreground: rgb(0xf0f6fc).into(),
            muted: rgb(0x9198a1).into(),
            border: rgb(0x3d444d).into(),
            button: rgb(0x212830).into(),
        }
    } else {
        Palette {
            background: rgb(0xffffff).into(),
            foreground: rgb(0x1f2328).into(),
            muted: rgb(0x59636e).into(),
            border: rgb(0xd1d9e0).into(),
            button: rgb(0xf6f8fa).into(),
        }
    }
}

/// One top-level piece of the README, and whether GitHub centres it.
#[derive(Debug, PartialEq)]
struct Block {
    text: SharedString,
    center: bool,
}

/// How many `<div>` and `<p>` tags `html` opens and closes.
fn tags(html: &str) -> (usize, usize) {
    let lower = html.to_ascii_lowercase();
    let count = |tag: &str| {
        lower
            .match_indices(tag)
            .filter(|(at, _)| matches!(lower[at + tag.len()..].chars().next(), Some(' ' | '>' | '\n' | '\t' | '/')))
            .count()
    };
    (count("<div") + count("<p"), count("</div") + count("</p"))
}

/// Whether the first `<div>` or `<p>` in `html` asks for its content centred.
fn centers(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<div").or_else(|| lower.find("<p")) else { return false };
    let tag = &lower[start..];
    let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
    tag.replace(['"', '\''], "").contains("align=center")
}

/// The README's blocks. Profile READMEs centre things with
/// `<div align="center">`, often with Markdown between the tags, which
/// Markdown keeps as separate blocks.
fn blocks(text: &str) -> Vec<Block> {
    let whole = || vec![Block { text: text.to_owned().into(), center: false }];
    let Ok(Node::Root(root)) = markdown::to_mdast(text, &markdown::ParseOptions::gfm()) else { return whole() };
    let source = |node: &Node| node.position().and_then(|at| text.get(at.start.offset..at.end.offset));
    // Each block is shown on its own, so each needs the link definitions.
    let definitions: String = root
        .children
        .iter()
        .filter(|node| matches!(node, Node::Definition(_)))
        .filter_map(source)
        .map(|definition| format!("\n\n{definition}"))
        .collect();
    // The tags still open, and whether each centres.
    let mut open: Vec<bool> = Vec::new();
    let mut out = Vec::new();
    for node in &root.children {
        let Some(source) = source(node) else { continue };
        let inside = open.contains(&true);
        match node {
            Node::Definition(_) => {}
            Node::Html(html) => {
                let (opened, closed) = tags(&html.value);
                let centered = inside || centers(&html.value);
                if closed > opened {
                    open.truncate(open.len().saturating_sub(closed - opened));
                } else {
                    open.extend(std::iter::repeat_n(centers(&html.value), opened - closed));
                }
                // A lone <div> or </div> shows nothing.
                let bare = opened + closed == 1 && html.value.trim().matches('<').count() == 1;
                if !bare {
                    out.push(Block { text: source.to_owned().into(), center: centered });
                }
            }
            _ => out.push(Block { text: format!("{source}{definitions}").into(), center: inside }),
        }
    }
    out
}

/// Where an image of `login`'s README comes from. GitHub shows images from
/// the web and from the repository, and never `data:` ones.
fn image_url(url: &str, login: &str) -> Option<String> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Some(url.to_owned());
    }
    // GitHub fetches these for the page; here they go over a secure line or not at all.
    if lower.starts_with("http://") {
        return Some(format!("https://{}", &url[7..]));
    }
    if url.contains(':') || url.starts_with("//") || url.starts_with('\\') {
        return None;
    }
    let path = url.trim_start_matches("./").trim_start_matches('/');
    Some(format!("https://raw.githubusercontent.com/{login}/{login}/HEAD/{path}"))
}

/// A link a user put on their profile, which may leave out `https://`.
fn web_link(link: &str) -> Option<String> {
    let link = link.trim();
    let lower = link.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("http://") {
        Some(link.to_owned())
    } else if !link.is_empty() && !link.contains(':') {
        Some(format!("https://{link}"))
    } else {
        None
    }
}

struct Loaded {
    profile: Profile,
    readme: Result<Option<Vec<Block>>, String>,
}

enum Load {
    Loading,
    Ready(Rc<Loaded>),
    Failed(String),
}

/// The profiles shown lately, and when GitHub sent each.
#[derive(Default)]
struct Profiles(HashMap<String, (Rc<Loaded>, Instant)>);

impl Global for Profiles {}

type OnAccount = Arc<dyn Fn(&mut Window, &mut App)>;

pub struct ProfileView {
    login: String,
    load: Load,
    /// Opens Somme's own account page, which this popup took the place of.
    on_account: OnAccount,
}

/// Shows `login`'s GitHub profile in a popup.
pub fn show(
    login: String,
    services: &Services,
    on_account: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let token = services.sync.github_token();
    let on_account: OnAccount = Arc::new(on_account);
    let view = cx.new(|cx| ProfileView::new(login, token, on_account, cx));
    let width = (window.viewport_size().width - px(64.)).min(px(1012.));
    window.open_dialog(cx, move |dialog, _, cx| dialog.w(width).bg(palette(cx.theme().is_dark()).background).child(view.clone()));
}

impl ProfileView {
    fn new(
        login: String,
        token: Option<Zeroizing<String>>,
        on_account: OnAccount,
        cx: &mut Context<Self>,
    ) -> Self {
        let kept = cx.try_global::<Profiles>().and_then(|profiles| profiles.0.get(&login).cloned());
        if let Some((loaded, at)) = &kept
            && at.elapsed() < KEEP
        {
            return Self {
                login,
                load: Load::Ready(loaded.clone()),
                on_account,
            };
        }
        let asked = login.clone();
        cx.spawn(async move |view, cx| {
            let loaded = runtime::io(async move {
                let ask = async |token: Zeroizing<String>| {
                    let client = GitHub::new(token)?;
                    let (profile, readme) = futures::join!(client.profile(&asked), client.profile_readme(&asked));
                    Ok::<_, ApiError>((profile?, readme))
                };
                match token {
                    // An old token is refused even for what is public.
                    Some(token) => match ask(token).await {
                        Err(ApiError::Unauthorized) => ask(Zeroizing::default()).await,
                        other => other,
                    },
                    None => ask(Zeroizing::default()).await,
                }
            })
            .await;
            let _ = view.update(cx, |view, cx| {
                view.load = match loaded {
                    Ok((profile, readme)) => {
                        let readme = readme.map(|readme| readme.map(|text| blocks(&text))).map_err(|err| err.to_string());
                        let loaded = Rc::new(Loaded { profile, readme });
                        cx.default_global::<Profiles>().0.insert(view.login.clone(), (loaded.clone(), Instant::now()));
                        Load::Ready(loaded)
                    }
                    // Say, too many asks this hour: what GitHub sent before beats nothing.
                    Err(err) => kept.map_or_else(|| Load::Failed(err.to_string()), |(loaded, _)| Load::Ready(loaded)),
                };
                cx.notify();
            });
        })
        .detach();
        Self {
            login,
            load: Load::Loading,
            on_account,
        }
    }

    fn page(&self) -> String {
        format!("https://github.com/{}", self.login)
    }

    fn render_details(&self, profile: &Profile, colors: &Palette) -> impl IntoElement {
        let page = self.page();
        let avatar = profile
            .avatar_url
            .as_deref()
            .filter(|url| url.starts_with("https://"))
            .map(|url| SharedUri::from(format!("{url}{}s=464", if url.contains('?') { "&" } else { "?" })));
        let line = |id: &'static str, icon: Lucide, text: String, link: Option<String>| {
            h_flex()
                .id(id)
                .gap_2()
                .items_start()
                .text_sm()
                .child(div().flex_none().pt(px(2.)).child(Icon::new(icon).size(px(16.)).text_color(colors.muted)))
                .child(div().flex_1().min_w_0().child(text))
                .when_some(link, |row, link| {
                    row.cursor_pointer().hover(|row| row.underline()).on_click(move |_, _, cx| cx.open_url(&link))
                })
        };
        let on_account = self.on_account.clone();
        v_flex()
            .w(px(AVATAR))
            .flex_none()
            .gap_4()
            .child(match avatar {
                Some(avatar) => img(avatar)
                    .size(px(AVATAR))
                    .rounded_full()
                    .border_1()
                    .border_color(colors.border)
                    .into_any_element(),
                None => div().size(px(AVATAR)).rounded_full().bg(colors.button).into_any_element(),
            })
            .child(
                v_flex()
                    .when_some(profile.name.clone().filter(|name| !name.is_empty()), |names, name| {
                        names.child(div().text_size(px(24.)).line_height(px(30.)).font_semibold().child(name))
                    })
                    .child(div().text_size(px(20.)).line_height(px(24.)).text_color(colors.muted).child(profile.login.clone())),
            )
            .child(
                Button::new("profile-open")
                    .w_full()
                    .small()
                    .label("View on GitHub")
                    .on_click(move |_, _, cx| cx.open_url(&page)),
            )
            .when_some(profile.bio.clone().filter(|bio| !bio.is_empty()), |el, bio| el.child(div().text_base().child(bio)))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .text_color(colors.muted)
                    .child(Icon::new(Lucide::Users).size(px(16.)))
                    .child(div().font_semibold().text_color(colors.foreground).child(profile.followers.to_string()))
                    .child(if profile.followers == 1 { "follower ·" } else { "followers ·" })
                    .child(div().font_semibold().text_color(colors.foreground).child(profile.following.to_string()))
                    .child("following"),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .when_some(profile.company.clone().filter(|it| !it.is_empty()), |el, company| {
                        el.child(line("profile-company", Lucide::Building2, company, None))
                    })
                    .when_some(profile.location.clone().filter(|it| !it.is_empty()), |el, location| {
                        el.child(line("profile-location", Lucide::MapPin, location, None))
                    })
                    .when_some(profile.blog.clone().filter(|it| !it.is_empty()), |el, blog| {
                        let link = web_link(&blog);
                        el.child(line("profile-blog", Lucide::Link, blog, link))
                    })
                    .when_some(profile.twitter_username.clone().filter(|it| !it.is_empty()), |el, name| {
                        let handle = name.len() <= 15 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                        let link = handle.then(|| format!("https://x.com/{name}"));
                        el.child(line("profile-x", Lucide::AtSign, name, link))
                    }),
            )
            .child(
                div().pt_2().border_t_1().border_color(colors.border).child(
                    Button::new("profile-account")
                        .ghost()
                        .xsmall()
                        .label("Somme account and sync")
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            on_account(window, cx);
                        }),
                ),
            )
            .into_any_element()
    }

    fn render_readme(&self, readme: &Result<Option<Vec<Block>>, String>, colors: &Palette) -> AnyElement {
        let login: Arc<str> = self.login.as_str().into();
        let body = match readme {
            Ok(Some(blocks)) => v_flex()
                .px_6()
                .pb_6()
                .gap_2()
                .children(blocks.iter().enumerate().map(|(ix, block)| {
                    let login = login.clone();
                    let text = TextView::markdown(("profile-md", ix), block.text.clone())
                        .selectable(true)
                        .image_source(move |uri: &SharedUri| {
                            image_url(uri.as_ref(), &login)
                                .map_or_else(|| ImageSource::from(std::path::PathBuf::new()), |url| ImageSource::from(SharedUri::from(url)))
                        });
                    div()
                        .w_full()
                        .when(block.center, |el| el.flex().flex_col().items_center().text_center())
                        .child(text)
                }))
                .into_any_element(),
            Ok(None) => div()
                .px_6()
                .pb_6()
                .text_sm()
                .text_color(colors.muted)
                .child(format!("{} has no profile README.", self.login))
                .into_any_element(),
            Err(err) => div().px_6().pb_6().text_sm().text_color(colors.muted).child(err.clone()).into_any_element(),
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .rounded(px(6.))
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(format!("{} / README.md", self.login)),
            )
            .child(body)
            .into_any_element()
    }
}

impl Render for ProfileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = palette(cx.theme().is_dark());
        let content = match &self.load {
            Load::Loading => h_flex()
                .h(px(420.))
                .justify_center()
                .items_center()
                .child(Spinner::new().large())
                .into_any_element(),
            Load::Failed(err) => {
                let page = self.page();
                v_flex()
                    .h(px(240.))
                    .justify_center()
                    .items_center()
                    .gap_3()
                    .child(div().text_sm().text_color(colors.muted).child(SharedString::from(err.clone())))
                    .child(Button::new("profile-open-failed").small().label("Open on GitHub").on_click(move |_, _, cx| cx.open_url(&page)))
                    .into_any_element()
            }
            Load::Ready(loaded) => {
                let Loaded { profile, readme } = &**loaded;
                let narrow = window.viewport_size().width < px(NARROW);
                let details = self.render_details(profile, &colors).into_any_element();
                let readme = self.render_readme(readme, &colors);
                if narrow {
                    v_flex().gap_6().child(details).child(readme).into_any_element()
                } else {
                    h_flex().gap_6().items_start().child(details).child(readme).into_any_element()
                }
            }
        };
        div().w_full().p_2().text_color(colors.foreground).child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_between_centred_tags_is_centred() {
        let text = "<div align=\"center\">\n\n**Hi**\n\n[![a](https://x.test/a.svg)](https://x.test)\n\n</div>\n\n### About\n\n<p align='center'><img src=\"b.png\"></p>\n\n<br/>\n";
        let got: Vec<(String, bool)> = blocks(text).into_iter().map(|block| (block.text.to_string(), block.center)).collect();
        assert_eq!(
            got,
            [
                ("**Hi**".to_owned(), true),
                ("[![a](https://x.test/a.svg)](https://x.test)".to_owned(), true),
                ("### About".to_owned(), false),
                ("<p align='center'><img src=\"b.png\"></p>".to_owned(), true),
                ("<br/>".to_owned(), false),
            ]
        );
    }

    #[test]
    fn images_come_from_the_web_or_the_repository() {
        assert_eq!(image_url("https://img.shields.io/x.svg", "me").as_deref(), Some("https://img.shields.io/x.svg"));
        assert_eq!(image_url("http://example.test/a.png", "me").as_deref(), Some("https://example.test/a.png"));
        assert_eq!(
            image_url("./assets/banner.gif", "me").as_deref(),
            Some("https://raw.githubusercontent.com/me/me/HEAD/assets/banner.gif")
        );
        for blocked in ["data:image/png;base64,AA", "file:///etc/passwd", r"\\server\a.png", "//host/a.png"] {
            assert_eq!(image_url(blocked, "me"), None, "{blocked}");
        }
        assert_eq!(web_link("stackoverflow.com/u/1").as_deref(), Some("https://stackoverflow.com/u/1"));
        assert_eq!(web_link("javascript:alert(1)"), None);
    }
}
