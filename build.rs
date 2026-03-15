// SPDX-FileCopyrightText: 2026 Alexander R. Croft
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=VERSION");
    println!("cargo:rerun-if-changed=BUILD");

    let version = read_trimmed("VERSION");
    let build = read_trimmed("BUILD");
    let display_version = format!("{version}+build.{build}");

    println!("cargo:rustc-env=RALLY_DISPLAY_VERSION={display_version}");
}

fn read_trimmed(path: &str) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {path}: {error}"))
        .trim()
        .to_owned()
}