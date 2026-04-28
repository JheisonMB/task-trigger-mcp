use super::*;

#[test]
fn test_to_7field_cron() {
    assert_eq!(to_7field_cron("*/5 * * * *"), "0 */5 * * * * *");
    assert_eq!(to_7field_cron("0 9 * * *"), "0 0 9 * * * *");
    assert_eq!(to_7field_cron("0 9 * * 1-5"), "0 0 9 * * 1-5 *");
}

#[test]
fn test_cron_parse_after_conversion() {
    let cases = vec![
        "*/5 * * * *",    // every 5 min
        "0 9 * * *",      // daily at 9am
        "0 9 * * 1-5",    // weekdays at 9am
        "30 14 1,15 * *", // 1st and 15th at 2:30pm
    ];

    for expr in cases {
        let converted = to_7field_cron(expr);
        let result = Schedule::from_str(&converted);
        assert!(
            result.is_ok(),
            "Failed to parse '{}' -> '{}': {:?}",
            expr,
            converted,
            result.err()
        );
    }
}

#[test]
fn test_to_7field_cron_trims_whitespace() {
    assert_eq!(to_7field_cron("  */5 * * * *  "), "0 */5 * * * * *");
    assert_eq!(to_7field_cron("\t0 9 * * *\t"), "0 0 9 * * * *");
}

#[test]
fn test_to_7field_cron_complex_expressions() {
    assert_eq!(to_7field_cron("0 */2 * * *"), "0 0 */2 * * * *");
    assert_eq!(to_7field_cron("15,30,45 * * * *"), "0 15,30,45 * * * *");
}

#[test]
fn test_cron_schedule_next_fire_time() {
    let converted = to_7field_cron("* * * * *");
    let schedule = Schedule::from_str(&converted).unwrap();
    let now = chrono::Utc::now();
    let next = schedule.after(&now).next();
    assert!(next.is_some());
}
