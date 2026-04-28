//! Tests for the autoupdate module

#[test]
fn compare_versions_equal() {
    assert_eq!(
        super::compare_versions("1.0.0", "1.0.0"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn compare_versions_greater_patch() {
    assert_eq!(
        super::compare_versions("1.0.1", "1.0.0"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn compare_versions_less_patch() {
    assert_eq!(
        super::compare_versions("1.0.0", "1.0.1"),
        std::cmp::Ordering::Less
    );
}

#[test]
fn compare_versions_major_wins() {
    assert_eq!(
        super::compare_versions("2.0.0", "1.9.9"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn compare_versions_with_v_prefix() {
    assert_eq!(
        super::compare_versions("v1.2.3", "1.2.3"),
        std::cmp::Ordering::Equal
    );
}

#[test]
fn compare_versions_different_length() {
    assert_eq!(
        super::compare_versions("1.0", "1.0.0"),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        super::compare_versions("1.0.0.1", "1.0.0"),
        std::cmp::Ordering::Greater
    );
}

#[test]
fn stable_version_accepts_plain() {
    assert!(super::is_stable_version("1.0.0"));
    assert!(super::is_stable_version("v1.0.0"));
    assert!(super::is_stable_version("v0.32.1"));
}

#[test]
fn stable_version_rejects_prerelease() {
    assert!(!super::is_stable_version("1.0.0-beta"));
    assert!(!super::is_stable_version("v1.0.0-rc1"));
    assert!(!super::is_stable_version("1.0.0-alpha+build123"));
}

#[test]
fn stable_version_rejects_empty() {
    assert!(!super::is_stable_version(""));
    assert!(!super::is_stable_version("v"));
}
