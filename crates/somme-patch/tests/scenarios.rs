// Changed by Somme from OpenAI Codex (codex-rs/apply-patch). Apache-2.0;
// see LICENSE and NOTICE in this crate.

//! Each folder in tests/fixtures/scenarios has the files before (input/),
//! a patch (patch.txt) and the files after (expected/).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use somme_patch::{LocalFiles, apply};

#[tokio::test]
async fn every_scenario_ends_with_the_expected_files() {
    let scenarios = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scenarios");
    let mut ran = 0;
    for scenario in fs::read_dir(&scenarios).unwrap() {
        let path = scenario.unwrap().path();
        if path.is_dir() {
            run(&path).await;
            ran += 1;
        }
    }
    assert!(ran > 20, "only {ran} scenarios");
}

async fn run(dir: &Path) {
    let tmp = tempfile::tempdir().unwrap();
    let input = dir.join("input");
    if input.is_dir() {
        copy_dir(&input, tmp.path());
    }
    let patch = fs::read_to_string(dir.join("patch.txt")).unwrap();
    // Scenarios only say how the files end up, not whether the patch failed.
    let _ = apply(&patch, &LocalFiles { root: tmp.path().to_path_buf() }).await;
    assert_eq!(snapshot(tmp.path()), snapshot(&dir.join("expected")), "scenario {}", dir.display());
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    File(Vec<u8>),
    Dir,
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut entries = BTreeMap::new();
    if root.is_dir() {
        walk(root, root, &mut entries);
    }
    entries
}

fn walk(base: &Path, dir: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(base).unwrap().to_path_buf();
        if path.is_dir() {
            entries.insert(relative, Entry::Dir);
            walk(base, &path, entries);
        } else {
            entries.insert(relative, Entry::File(fs::read(&path).unwrap()));
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() {
            fs::create_dir_all(&dest).unwrap();
            copy_dir(&path, &dest);
        } else {
            fs::copy(&path, &dest).unwrap();
        }
    }
}
