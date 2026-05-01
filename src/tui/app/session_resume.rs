pub(crate) fn args_contain_flag(args: &str, flag: &str) -> bool {
    args.split_whitespace().any(|arg| arg == flag)
}

pub(crate) fn append_flag_if_missing(
    base_args: Option<&str>,
    yolo_flag: Option<&str>,
    should_include_yolo: bool,
) -> Option<String> {
    let base = base_args.map(str::trim).filter(|args| !args.is_empty());

    match (base, yolo_flag, should_include_yolo) {
        (Some(args), Some(flag), true) if !args_contain_flag(args, flag) => {
            Some(format!("{args} {flag}"))
        }
        (Some(args), _, _) => Some(args.to_string()),
        (None, Some(flag), true) => Some(flag.to_string()),
        (None, _, _) => None,
    }
}

pub(crate) fn build_resumed_session_args(
    session: &crate::db::session::InteractiveSession,
    interactive_args: Option<&str>,
    yolo_flag: Option<&str>,
) -> Option<String> {
    let original_args = session
        .args
        .as_deref()
        .map(str::trim)
        .filter(|args| !args.is_empty());
    let inter_args = interactive_args
        .map(str::trim)
        .filter(|args| !args.is_empty());
    let had_yolo = yolo_flag
        .is_some_and(|flag| original_args.is_some_and(|args| args_contain_flag(args, flag)));

    // Prefer original args (they were already constructed by launch_interactive).
    // If none were persisted (legacy session), fall back to interactive_args from config.
    append_flag_if_missing(original_args.or(inter_args), yolo_flag, had_yolo)
}
