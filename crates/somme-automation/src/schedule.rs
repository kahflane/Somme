//! Schedules: cron times like `0 9 * * 1-5`, and the runs the computer slept
//! through.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike as _, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike as _};

use crate::schema::{Missed, Start};

/// A run this late is still on time, as when the computer woke a minute after.
const GRACE: Duration = Duration::minutes(3);

const MONTHS: &[&str] = &["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
const DAYS: &[&str] = &["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// When a schedule runs: five cron fields, as bits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    /// Sunday is 0.
    weekdays: u64,
    /// The day or weekday field starts with `*`: then a day must match both.
    day_star: bool,
    weekday_star: bool,
}

impl Cron {
    /// Five fields (minute, hour, day, month, weekday), or `@hourly`,
    /// `@daily`, `@weekly`, `@monthly`, `@yearly`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let lower = text.trim().to_lowercase();
        let text = match lower.as_str() {
            "@hourly" => "0 * * * *",
            "@daily" | "@midnight" => "0 0 * * *",
            "@weekly" => "0 0 * * 0",
            "@monthly" => "0 0 1 * *",
            "@yearly" | "@annually" => "0 0 1 1 *",
            other => other,
        };
        let fields: Vec<&str> = text.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err("A schedule has five parts: minute, hour, day, month and weekday, like 0 9 * * 1-5.".into());
        };
        let weekdays = field(weekday, 0, 7, DAYS, 0, "weekday")?;
        Ok(Self {
            minutes: field(minute, 0, 59, &[], 0, "minute")?,
            hours: field(hour, 0, 23, &[], 0, "hour")?,
            days: field(day, 1, 31, &[], 0, "day")?,
            months: field(month, 1, 12, MONTHS, 1, "month")?,
            // 7 is Sunday too.
            weekdays: (weekdays | (weekdays >> 7)) & 0x7f,
            day_star: day.starts_with('*'),
            weekday_star: weekday.starts_with('*'),
        })
    }

    /// The first time after `after`, to the minute.
    pub fn next_after<Tz: TimeZone>(&self, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        let zone = after.timezone();
        let mut at: NaiveDateTime = after.naive_local().with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        // Over four years of days, so a schedule for 29 February is found.
        for _ in 0..20_000 {
            if self.months & (1 << at.month()) == 0 {
                let (year, month) = if at.month() == 12 { (at.year() + 1, 1) } else { (at.year(), at.month() + 1) };
                at = NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.on_day(at.date()) {
                at = at.date().succ_opt()?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if self.hours & (1 << at.hour()) == 0 {
                at = at.with_minute(0)? + Duration::hours(1);
                continue;
            }
            if self.minutes & (1 << at.minute()) == 0 {
                at += Duration::minutes(1);
                continue;
            }
            let found = match zone.from_local_datetime(&at) {
                LocalResult::Single(found) | LocalResult::Ambiguous(found, _) => Some(found),
                // The clock jumped past it.
                LocalResult::None => None,
            };
            match found {
                Some(found) if found > *after => return Some(found),
                _ => at += Duration::minutes(1),
            }
        }
        None
    }

    fn on_day(&self, date: NaiveDate) -> bool {
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().num_days_from_sunday()) != 0;
        if self.day_star || self.weekday_star { day && weekday } else { day || weekday }
    }
}

/// One cron field as bits. `names` stand for `first`, `first + 1`, ….
fn field(text: &str, low: u32, high: u32, names: &[&str], first: u32, what: &str) -> Result<u64, String> {
    let bad = || format!("\u{201c}{text}\u{201d} is not a {what} a schedule understands.");
    let value = |part: &str| -> Result<u32, String> {
        if let Some(ix) = names.iter().position(|name| name.eq_ignore_ascii_case(part)) {
            return Ok(ix as u32 + first);
        }
        part.parse::<u32>().ok().filter(|n| (low..=high).contains(n)).ok_or_else(bad)
    };
    let mut bits = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<usize>().ok().filter(|step| *step > 0).ok_or_else(bad)?),
            None => (part, 1),
        };
        let (from, to) = if range == "*" {
            (low, high)
        } else if let Some((from, to)) = range.split_once('-') {
            (value(from)?, value(to)?)
        } else {
            let from = value(range)?;
            (from, if step > 1 { high } else { from })
        };
        if from > to {
            return Err(bad());
        }
        for n in (from..=to).step_by(step) {
            bits |= 1 << n;
        }
    }
    Ok(bits)
}

/// A schedule in words when it is a common one, like `Weekdays at 9:00`;
/// else the schedule itself.
pub fn words(text: &str) -> String {
    let text = text.trim();
    let named = match text.to_lowercase().as_str() {
        "* * * * *" => Some("Every minute".to_owned()),
        "@hourly" | "0 * * * *" => Some("Every hour".to_owned()),
        "@daily" | "@midnight" => Some("Every day at 0:00".to_owned()),
        "@weekly" => Some("Sundays at 0:00".to_owned()),
        "@monthly" => Some("The 1st of each month at 0:00".to_owned()),
        _ => None,
    };
    if let Some(named) = named {
        return named;
    }
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [minute, hour, "*", "*", weekday] = fields.as_slice() else { return text.to_owned() };
    let (Ok(minute), Ok(hour)) = (minute.parse::<u32>(), hour.parse::<u32>()) else {
        if let (Some(step), ["*", "*"]) = (minute.strip_prefix("*/"), [*hour, *weekday]) {
            return format!("Every {step} minutes");
        }
        return text.to_owned();
    };
    if minute > 59 || hour > 23 {
        return text.to_owned();
    }
    let at = format!("{hour}:{minute:02}");
    const PLURAL: [&str; 7] = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"];
    match *weekday {
        "*" => format!("Every day at {at}"),
        "1-5" => format!("Weekdays at {at}"),
        "0,6" | "6,0" => format!("Weekends at {at}"),
        day => match day.parse::<usize>() {
            Ok(day) if day <= 7 => format!("{} at {at}", PLURAL[day % 7]),
            _ => text.to_owned(),
        },
    }
}

/// Whether a schedule has a run due between `checked` and `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    No,
    Now,
    /// It was due while the computer slept.
    Missed,
}

pub fn due<Tz: TimeZone>(cron: &Cron, checked: &DateTime<Tz>, now: &DateTime<Tz>) -> Due {
    let Some(mut latest) = cron.next_after(checked).filter(|first| first <= now) else { return Due::No };
    // Only the newest one counts: older ones were missed too.
    for _ in 0..10_000 {
        match cron.next_after(&latest) {
            Some(next) if next <= *now => latest = next,
            _ => break,
        }
    }
    if now.clone() - latest <= GRACE { Due::Now } else { Due::Missed }
}

/// The automations to run now, by id, and when each was checked, for next
/// time. `scheduled` gives each automation's start; one checked for the first
/// time starts counting now.
pub fn plan<Tz: TimeZone>(scheduled: &[(String, Start)], checked: &BTreeMap<String, i64>, now: &DateTime<Tz>) -> (Vec<String>, BTreeMap<String, i64>) {
    let mut run = Vec::new();
    let mut next = BTreeMap::new();
    let zone = now.timezone();
    for (id, start) in scheduled {
        let Some(cron) = start.schedule.as_deref().filter(|text| !text.trim().is_empty()).and_then(|text| Cron::parse(text).ok()) else { continue };
        if let Some(last) = checked.get(id).and_then(|ms| zone.timestamp_millis_opt(*ms).single()) {
            match due(&cron, &last, now) {
                Due::Now => run.push(id.clone()),
                Due::Missed if start.missed == Missed::RunOnce => run.push(id.clone()),
                Due::Missed | Due::No => {}
            }
        }
        next.insert(id.clone(), now.timestamp_millis());
    }
    (run, next)
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap().and_utc()
    }

    fn next(cron: &str, after: &str) -> String {
        Cron::parse(cron).unwrap().next_after(&at(after)).unwrap().format("%Y-%m-%d %H:%M %a").to_string()
    }

    #[test]
    fn schedules_find_their_next_time() {
        assert_eq!(next("0 * * * *", "2026-10-19 10:00"), "2026-10-19 11:00 Mon");
        assert_eq!(next("*/15 * * * *", "2026-10-19 10:07"), "2026-10-19 10:15 Mon");
        assert_eq!(next("0 9 * * 1-5", "2026-10-23 09:00"), "2026-10-26 09:00 Mon");
        assert_eq!(next("30 2 1 jan *", "2026-10-19 10:00"), "2027-01-01 02:30 Fri");
        assert_eq!(next("0 0 29 2 *", "2026-03-01 00:00"), "2028-02-29 00:00 Tue");
        assert_eq!(next("@weekly", "2026-10-19 10:00"), "2026-10-25 00:00 Sun");
        assert_eq!(next("0 0 * * 7", "2026-10-19 10:00"), "2026-10-25 00:00 Sun");
        // Both day fields set: either one will do.
        assert_eq!(next("0 0 1 * mon", "2026-10-19 10:00"), "2026-10-26 00:00 Mon");
        assert_eq!(next("0 12 */10 * *", "2026-10-19 10:00"), "2026-10-21 12:00 Wed");
    }

    #[test]
    fn common_schedules_read_as_words() {
        assert_eq!(words("0 * * * *"), "Every hour");
        assert_eq!(words("0 9 * * 1-5"), "Weekdays at 9:00");
        assert_eq!(words("30 2 * * *"), "Every day at 2:30");
        assert_eq!(words("0 9 * * 1"), "Mondays at 9:00");
        assert_eq!(words("*/15 * * * *"), "Every 15 minutes");
        assert_eq!(words("0 9 1 * *"), "0 9 1 * *");
    }

    #[test]
    fn bad_schedules_say_why() {
        assert!(Cron::parse("0 9 * *").unwrap_err().contains("five parts"));
        assert!(Cron::parse("61 * * * *").unwrap_err().contains("minute"));
        assert!(Cron::parse("0 9 * * fun").unwrap_err().contains("weekday"));
        assert!(Cron::parse("0 9-5 * * *").is_err());
    }

    #[test]
    fn a_run_missed_in_sleep_runs_once_or_not_at_all() {
        let hourly = Cron::parse("0 * * * *").unwrap();
        assert_eq!(due(&hourly, &at("2026-10-19 10:00"), &at("2026-10-19 10:30")), Due::No);
        assert_eq!(due(&hourly, &at("2026-10-19 10:59"), &at("2026-10-19 11:01")), Due::Now);
        assert_eq!(due(&hourly, &at("2026-10-19 10:59"), &at("2026-10-19 14:30")), Due::Missed);

        let start = |missed| Start { schedule: Some("0 * * * *".into()), missed, agents: false };
        let scheduled = vec![("a".to_owned(), start(Missed::RunOnce)), ("b".to_owned(), start(Missed::Skip)), ("new".to_owned(), start(Missed::RunOnce))];
        let checked = BTreeMap::from([("a".to_owned(), at("2026-10-19 08:10").timestamp_millis()), ("b".to_owned(), at("2026-10-19 08:10").timestamp_millis()), ("gone".to_owned(), 1)]);
        let now = at("2026-10-19 14:30");
        let (run, next) = plan(&scheduled, &checked, &now);
        assert_eq!(run, ["a"]);
        assert_eq!(next.keys().collect::<Vec<_>>(), ["a", "b", "new"]);
        assert!(next.values().all(|ms| *ms == now.timestamp_millis()));
    }
}
