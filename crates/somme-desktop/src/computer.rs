//! Agents' use of this computer's screen, mouse and keys. One agent at a
//! time, in its step's apps only, while a bar at the top of the screen says
//! so. Esc or the bar's Stop takes control back at once. Screenshots go to
//! the LLM and are not kept.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::channel::mpsc::UnboundedSender;
use parking_lot::Mutex;
use somme_automation::computer::{self as screen, Action, Button, STOPPED, Seen};
use somme_computer::{Input, Mouse, Platform};

use crate::ui::common::APP_NAME;

/// Screenshots are made at most this wide for the LLM.
const SHOT_WIDTH: u32 = 1280;
/// How long the screen gets to settle after input, before it is looked at.
const SETTLE: Duration = Duration::from_millis(500);
/// How long an app gets to open.
const OPENING: Duration = Duration::from_millis(1500);
/// How often Esc is checked.
const ESC_EVERY: Duration = Duration::from_millis(50);

/// What the bar at the top of the screen shows.
pub enum Banner {
    /// `who` uses the screen; setting `stop` takes it back.
    Show { who: String, stop: Arc<AtomicBool> },
    Hide,
}

/// Only one agent uses the screen at a time.
static IN_USE: AtomicBool = AtomicBool::new(false);

/// The screen agents use: the real one, or a pretend one for scripted UI
/// checks, which never touch the real screen.
pub fn platform() -> Result<&'static dyn Platform, String> {
    #[cfg(feature = "snapshot")]
    if std::env::var_os("SOMME_SCRIPT").is_some() {
        return Ok(&pretend::Pretend);
    }
    somme_computer::system()
}

/// Points in the last screenshot, mapped onto the screen.
#[derive(Clone, Copy, Debug)]
struct Map {
    x: i32,
    y: i32,
    across: f64,
    down: f64,
}

impl Map {
    fn to_screen(self, x: i32, y: i32) -> (i32, i32) {
        (self.x + (f64::from(x) * self.across).round() as i32, self.y + (f64::from(y) * self.down).round() as i32)
    }
}

/// One agent's hold on the screen. Dropping it gives the screen back.
pub struct Session {
    platform: &'static dyn Platform,
    apps: Vec<String>,
    stop: Arc<AtomicBool>,
    over: Arc<AtomicBool>,
    /// True while the agent itself presses Esc.
    own_escape: Arc<AtomicBool>,
    map: Mutex<Option<Map>>,
    banner: UnboundedSender<Banner>,
}

impl Session {
    pub fn start(apps: &[String], who: &str, banner: UnboundedSender<Banner>) -> Result<Arc<Self>, String> {
        let platform = platform()?;
        platform.ready()?;
        if IN_USE.swap(true, Ordering::SeqCst) {
            return Err("Another agent is using this computer's screen. Run this one when it is done.".into());
        }
        let session = Arc::new(Self {
            platform,
            apps: apps.to_vec(),
            stop: Arc::default(),
            over: Arc::default(),
            own_escape: Arc::default(),
            map: Mutex::new(None),
            banner,
        });
        let (stop, over, own_escape) = (session.stop.clone(), session.over.clone(), session.own_escape.clone());
        let watching = std::thread::Builder::new().name("somme-esc".into()).spawn(move || {
            while !over.load(Ordering::SeqCst) {
                if !own_escape.load(Ordering::SeqCst) && platform.escape_down() {
                    stop.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(ESC_EVERY);
            }
        });
        if watching.is_err() {
            session.end();
            return Err(format!("{APP_NAME} could not watch for Esc, so it won't take the screen."));
        }
        let _ = session.banner.unbounded_send(Banner::Show { who: who.to_owned(), stop: session.stop.clone() });
        Ok(session)
    }

    /// True once the user took back control.
    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Does one thing on the screen and looks at it after.
    pub async fn act(self: &Arc<Self>, action: Action) -> Result<Seen, String> {
        if self.stopped() {
            return Err(STOPPED.into());
        }
        let session = self.clone();
        tokio::task::spawn_blocking(move || session.act_now(&action)).await.map_err(|_| "The screen action stopped part way.".to_owned())?
    }

    fn act_now(&self, action: &Action) -> Result<Seen, String> {
        match action {
            Action::Screenshot => {}
            Action::Wait { seconds } => self.wait(Duration::from_secs_f64(*seconds)),
            Action::OpenApp { name } => {
                self.may_use(Some(name.clone()))?;
                self.platform.open_app(name)?;
                self.wait(OPENING);
            }
            _ => {
                let map = (*self.map.lock()).ok_or("Take a screenshot first, so points mean something.")?;
                let input = self.input(action, map)?;
                if self.stopped() {
                    return Err(STOPPED.into());
                }
                let escape = matches!(action, Action::Key { keys } if somme_computer::parse_keys(keys).is_ok_and(|keys| keys.iter().any(somme_computer::is_escape)));
                self.own_escape.store(escape, Ordering::SeqCst);
                let sent = self.platform.send(&input);
                if escape {
                    std::thread::sleep(ESC_EVERY * 3);
                    self.own_escape.store(false, Ordering::SeqCst);
                }
                sent?;
                self.wait(SETTLE);
            }
        }
        if self.stopped() {
            return Err(STOPPED.into());
        }
        self.look()
    }

    /// The input for an action, once the app it goes to may be used.
    fn input(&self, action: &Action, map: Map) -> Result<Vec<Input>, String> {
        let mouse = |button: &Button| match button {
            Button::Left => Mouse::Left,
            Button::Right => Mouse::Right,
            Button::Middle => Mouse::Middle,
        };
        // Clicks in the menu bar go to the app in front.
        let app_at = |x: i32, y: i32| self.platform.app_at(x, y).or_else(|| self.platform.front_app());
        Ok(match action {
            Action::Click { x, y, button, double, .. } => {
                let (x, y) = map.to_screen(*x, *y);
                self.may_use(app_at(x, y))?;
                let mut input = vec![Input::MoveTo(x, y), Input::Click(mouse(button))];
                if *double {
                    input.push(Input::Click(mouse(button)));
                }
                input
            }
            Action::Move { x, y } => {
                let (x, y) = map.to_screen(*x, *y);
                vec![Input::MoveTo(x, y)]
            }
            Action::Drag { x, y, to_x, to_y } => {
                let (x, y) = map.to_screen(*x, *y);
                let (to_x, to_y) = map.to_screen(*to_x, *to_y);
                self.may_use(app_at(x, y))?;
                self.may_use(app_at(to_x, to_y))?;
                let pause = Input::Pause(Duration::from_millis(100));
                vec![
                    Input::MoveTo(x, y),
                    Input::Press(Mouse::Left),
                    pause.clone(),
                    Input::MoveTo((x + to_x) / 2, (y + to_y) / 2),
                    Input::MoveTo(to_x, to_y),
                    pause,
                    Input::Release(Mouse::Left),
                ]
            }
            Action::Scroll { x, y, down, right } => {
                let (x, y) = map.to_screen(*x, *y);
                self.may_use(app_at(x, y))?;
                vec![Input::MoveTo(x, y), Input::Scroll { down: *down, right: *right }]
            }
            Action::Type { text } => {
                self.may_use(self.platform.front_app())?;
                if self.platform.in_password_box() {
                    return Err("The cursor is in a password box. Agents never type there; ask the user to do it.".into());
                }
                vec![Input::Text(text.clone())]
            }
            Action::Key { keys } => {
                self.may_use(self.platform.front_app())?;
                let keys = somme_computer::parse_keys(keys)?;
                let mut input: Vec<Input> = keys.iter().map(|key| Input::KeyDown(*key)).collect();
                input.extend(keys.iter().rev().map(|key| Input::KeyUp(*key)));
                input
            }
            Action::Screenshot | Action::Wait { .. } | Action::OpenApp { .. } => Vec::new(),
        })
    }

    /// Ok when input may go to `app`.
    fn may_use(&self, app: Option<String>) -> Result<(), String> {
        let Some(app) = app else {
            return Err(format!("{APP_NAME} can't tell which app is there, so it won't touch it."));
        };
        if app.eq_ignore_ascii_case(APP_NAME) || app.eq_ignore_ascii_case(&somme_computer::own_name()) {
            return Err(format!("That is {APP_NAME} itself. Agents never use {APP_NAME}'s own windows."));
        }
        if !screen::allowed(&self.apps, &app) {
            return Err(format!("{app} is not one of this step's apps ({}). Bring one to the front with open_app.", self.apps.join(", ")));
        }
        Ok(())
    }

    fn look(&self) -> Result<Seen, String> {
        let shot = self.platform.capture()?;
        let (png, width, height) = shot.png(SHOT_WIDTH)?;
        *self.map.lock() = Some(Map {
            x: shot.x,
            y: shot.y,
            across: f64::from(shot.width) / f64::from(width.max(1)),
            down: f64::from(shot.height) / f64::from(height.max(1)),
        });
        Ok(Seen { png, width, height, app: self.platform.front_app().unwrap_or_else(|| "no app".into()) })
    }

    /// Waits, but not past a stop.
    fn wait(&self, time: Duration) {
        let until = Instant::now() + time;
        while !self.stopped() {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(ESC_EVERY));
        }
    }

    /// Gives the screen back.
    pub fn end(&self) {
        if !self.over.swap(true, Ordering::SeqCst) {
            IN_USE.store(false, Ordering::SeqCst);
            let _ = self.banner.unbounded_send(Banner::Hide);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.end();
    }
}

/// A plain picture for a screen, and input that goes nowhere.
#[cfg(feature = "snapshot")]
mod pretend {
    use somme_computer::{Input, Platform, Rgba, RgbaImage, Shot};

    pub struct Pretend;

    impl Platform for Pretend {
        fn ready(&self) -> Result<(), String> {
            Ok(())
        }

        fn capture(&self) -> Result<Shot, String> {
            let mut image = RgbaImage::from_pixel(1280, 800, Rgba([236, 239, 244, 255]));
            for x in 560..720 {
                for y in 380..420 {
                    image.put_pixel(x, y, Rgba([59, 130, 246, 255]));
                }
            }
            Ok(Shot { image, x: 0, y: 0, width: 1280, height: 800 })
        }

        fn front_app(&self) -> Option<String> {
            Some("Safari".into())
        }

        fn app_at(&self, _: i32, _: i32) -> Option<String> {
            Some("Safari".into())
        }

        fn escape_down(&self) -> bool {
            false
        }

        fn open_app(&self, name: &str) -> Result<(), String> {
            eprintln!("pretend screen: opens {name}");
            Ok(())
        }

        fn send(&self, input: &[Input]) -> Result<(), String> {
            eprintln!("pretend screen: {input:?}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_in_a_small_screenshot_land_on_the_screen() {
        let map = Map { x: -1440, y: 0, across: 2.0, down: 2.0 };
        assert_eq!(map.to_screen(100, 50), (-1240, 100));
    }
}
