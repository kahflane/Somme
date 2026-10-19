//! The bar at the top of the screen while an agent uses this computer's
//! screen. It floats over every app, never takes the keyboard, and its Stop
//! takes control back, like Esc.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _, Render, Styled as _, Window,
    WindowBounds, WindowKind, WindowOptions, div, point, px, size,
};
use parking_lot::Mutex;

use crate::computer::Banner;
use crate::ui::common::{APP_NAME, Lucide, ink_button};

const WIDTH: f32 = 560.;
const HEIGHT: f32 = 58.;

/// The bar's window while it is up.
static BAR: Mutex<Option<AnyWindowHandle>> = Mutex::new(None);

/// Shows and hides the bar as the screen is taken and given back.
pub fn init(mut banners: UnboundedReceiver<Banner>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(banner) = banners.next().await {
            cx.update(|cx| {
                close(cx);
                if let Banner::Show { who, stop } = banner {
                    open(who, stop, cx);
                }
            });
        }
    })
    .detach();
}

/// The bar's window, if it is up.
#[cfg(feature = "snapshot")]
pub fn window() -> Option<AnyWindowHandle> {
    *BAR.lock()
}

/// True for the bar's window, which is never the place to ask the user.
pub fn is_bar(window: &AnyWindowHandle) -> bool {
    BAR.lock().is_some_and(|bar| bar.window_id() == window.window_id())
}

fn close(cx: &mut App) {
    if let Some(bar) = BAR.lock().take() {
        let _ = bar.update(cx, |_, window, _| window.remove_window());
    }
}

fn open(who: String, stop: Arc<AtomicBool>, cx: &mut App) {
    let display = cx.primary_display();
    let area = display.as_ref().map(|display| display.visible_bounds());
    let origin = area.map_or(point(px(0.), px(0.)), |area| point(area.center().x - px(WIDTH / 2.), area.origin.y + px(8.)));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(WIDTH), px(HEIGHT)) })),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id: display.map(|display| display.id()),
        ..WindowOptions::default()
    };
    match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| ControlBar { who, stop })) {
        Ok((window, _)) => *BAR.lock() = Some(window),
        Err(err) => tracing::warn!(%err, "could not show the bar that says an agent uses the screen"),
    }
}

struct ControlBar {
    who: String,
    stop: Arc<AtomicBool>,
}

impl Render for ControlBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let stopped = self.stop.load(Ordering::SeqCst);
        let said = if stopped { "Stopping\u{2026}".to_owned() } else { format!("{APP_NAME} is controlling this computer. Press Esc to stop.") };
        h_flex()
            .size_full()
            .px_3()
            .gap_2()
            .items_center()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_lg()
            .shadow_lg()
            .child(Icon::new(Lucide::Bot).text_color(theme.foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_sm().child(said))
                    .child(div().truncate().text_xs().text_color(theme.muted_foreground).child(format!("For {}", self.who))),
            )
            .child(
                Button::new("control-stop")
                    .custom(ink_button(cx))
                    .small()
                    .label("Stop")
                    .disabled(stopped)
                    .on_click(cx.listener(|bar, _, _, cx| {
                        bar.stop.store(true, Ordering::SeqCst);
                        cx.notify();
                    })),
            )
    }
}
