//! Drives the app from a script and saves screenshots, for checking the UI
//! without a person at the keyboard. Only built with `--features snapshot`.
//!
//! Run with `SOMME_SCRIPT=steps.txt`. One step per line:
//! `wait 500`, `key cmd-t`, `type hello`, `click 120 40`, `move 120 40`, `drag 10 10 200 10`, `action workspace::NewTab`,
//! `shot out.png`, `quit`. Lines starting with `#` are skipped.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{
    AnyWindowHandle, App, AsyncApp, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
    point, px,
};

pub fn start(window: AnyWindowHandle, cx: &mut App) {
    let Some(script) = std::env::var_os("SOMME_SCRIPT").map(PathBuf::from) else {
        return;
    };
    let steps = match std::fs::read_to_string(&script) {
        Ok(text) => text,
        Err(err) => {
            eprintln!("automation: cannot read {}: {err}", script.display());
            return;
        }
    };
    let base = script.parent().map(Path::to_path_buf).unwrap_or_default();
    cx.spawn(async move |cx| {
        for (number, line) in steps.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Err(err) = run_step(line, &base, window, cx).await {
                eprintln!("automation: line {}: {line}: {err}", number + 1);
            }
        }
    })
    .detach();
}

async fn run_step(line: &str, base: &Path, window: AnyWindowHandle, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let (command, arg) = line.split_once(' ').unwrap_or((line, ""));
    match command {
        "wait" => {
            let ms: u64 = arg.trim().parse()?;
            cx.background_executor().timer(Duration::from_millis(ms)).await;
        }
        "key" => {
            for stroke in arg.split_whitespace() {
                let keystroke = Keystroke::parse(stroke)?;
                window.update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))?;
                cx.background_executor().timer(Duration::from_millis(30)).await;
            }
        }
        "type" => {
            for ch in arg.chars() {
                let text = ch.to_string();
                let keystroke = Keystroke {
                    modifiers: Modifiers::default(),
                    key: if ch == ' ' { "space".into() } else { text.clone() },
                    key_char: Some(text),
                };
                window.update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx))?;
            }
        }
        "click" | "rclick" | "dclick" => {
            let mut parts = arg.split_whitespace().map(str::parse::<f32>);
            let (Some(Ok(x)), Some(Ok(y))) = (parts.next(), parts.next()) else {
                anyhow::bail!("expected: click X Y");
            };
            let button = if command == "rclick" { MouseButton::Right } else { MouseButton::Left };
            let clicks = if command == "dclick" { 2 } else { 1 };
            let position = point(px(x), px(y));
            // Click handlers only listen for the mouse-up after a frame has
            // seen the mouse-down, so draw a frame before each event. A window
            // hidden behind others does not redraw on its own.
            let pause = Duration::from_millis(40);
            window.update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })?;
            cx.background_executor().timer(pause).await;
            for click_count in 1..=clicks {
                window.update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button,
                            position,
                            modifiers: Modifiers::default(),
                            click_count,
                            first_mouse: false,
                        }),
                        cx,
                    );
                })?;
                cx.background_executor().timer(pause).await;
                window.update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    window.dispatch_event(
                        PlatformInput::MouseUp(MouseUpEvent {
                            button,
                            position,
                            modifiers: Modifiers::default(),
                            click_count,
                        }),
                        cx,
                    );
                })?;
                cx.background_executor().timer(pause).await;
            }
        }
        "drag" => {
            let mut parts = arg.split_whitespace().map(str::parse::<f32>);
            let (Some(Ok(x1)), Some(Ok(y1)), Some(Ok(x2)), Some(Ok(y2))) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                anyhow::bail!("expected: drag X1 Y1 X2 Y2");
            };
            let pause = Duration::from_millis(40);
            let start = point(px(x1), px(y1));
            let end = point(px(x2), px(y2));
            let send = |event: PlatformInput, cx: &mut AsyncApp| {
                window.update(cx, |_, window, cx| {
                    window.draw(cx).clear(cx);
                    window.dispatch_event(event, cx);
                })
            };
            send(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: start,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            )?;
            cx.background_executor().timer(pause).await;
            send(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: start,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            )?;
            cx.background_executor().timer(pause).await;
            // Several moves, so the drag starts and drop targets see the cursor arrive.
            const STEPS: usize = 8;
            for step in 1..=STEPS {
                let t = step as f32 / STEPS as f32;
                send(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(x1 + (x2 - x1) * t), px(y1 + (y2 - y1) * t)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                )?;
                cx.background_executor().timer(pause).await;
            }
            send(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: end,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            )?;
            cx.background_executor().timer(pause).await;
        }
        "move" => {
            let mut parts = arg.split_whitespace().map(str::parse::<f32>);
            let (Some(Ok(x)), Some(Ok(y))) = (parts.next(), parts.next()) else {
                anyhow::bail!("expected: move X Y");
            };
            window.update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(x), px(y)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })?;
        }
        "action" => {
            let name = arg.trim().to_owned();
            window.update(cx, |_, window, cx| -> anyhow::Result<()> {
                let action = cx.build_action(&name, None)?;
                window.dispatch_action(action, cx);
                Ok(())
            })??;
        }
        "shot" => {
            let path = base.join(arg.trim());
            let image = window.update(cx, |_, window, cx| {
                window.draw(cx).clear(cx);
                window.render_to_image()
            })??;
            image.save(&path)?;
            eprintln!("automation: saved {}", path.display());
        }
        "quit" => {
            cx.update(|cx| cx.quit());
        }
        other => anyhow::bail!("unknown step `{other}`"),
    }
    Ok(())
}
