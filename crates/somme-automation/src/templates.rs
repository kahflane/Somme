//! The automations Somme ships, to start from.

use crate::layout;
use crate::schema::Automation;

const FILES: &[&str] = &[
    include_str!("../templates/server-health.json"),
    include_str!("../templates/safe-deploy.json"),
    include_str!("../templates/incident-first-look.json"),
    include_str!("../templates/database-backup.json"),
    include_str!("../templates/security-updates.json"),
    include_str!("../templates/new-server-setup.json"),
    include_str!("../templates/disk-cleanup.json"),
    include_str!("../templates/certificate-watch.json"),
    include_str!("../templates/fix-it-for-me.json"),
];

/// Each template, laid out.
pub fn all() -> Vec<Automation> {
    FILES
        .iter()
        .filter_map(|text| Automation::from_json(text).ok())
        .map(|mut automation| {
            layout::tidy(&mut automation);
            automation
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::problems;

    #[test]
    fn every_template_reads_and_could_run() {
        let all = all();
        assert_eq!(all.len(), FILES.len());
        for automation in &all {
            assert!(problems(automation).is_empty(), "{}: {:?}", automation.name, problems(automation));
            assert!(!automation.description.is_empty(), "{}", automation.name);
        }
    }
}
