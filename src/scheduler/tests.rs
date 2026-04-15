use super::*;


    #[test]
    fn test_substitute_variables() {
        let prompt = "BackgroundAgent {{TASK_ID}} at {{TIMESTAMP}} on {{FILE_PATH}}";
        let result = substitute_variables(
            prompt,
            "my-background_agent",
            "/logs/my-background_agent.log",
            Some("/home/file.txt"),
            None,
        );
        assert!(result.contains("my-background_agent"));
        assert!(result.contains("/home/file.txt"));
        assert!(!result.contains("{{TIMESTAMP}}"));
    }

    #[test]
    fn test_validate_cron_valid() {
        assert!(validate_cron("*/5 * * * *"));
        assert!(validate_cron("0 9 * * *"));
        assert!(validate_cron("0 9 * * 1-5"));
        assert!(validate_cron("30 14 1,15 * *"));
        assert!(validate_cron("0 */2 * * *"));
    }

    #[test]
    fn test_validate_cron_invalid() {
        assert!(!validate_cron("every 5 minutes"));
        assert!(!validate_cron("daily at 9am"));
        assert!(!validate_cron("* * *")); // only 3 fields
        assert!(!validate_cron("")); // empty
        assert!(!validate_cron("0 9 * * * *")); // 6 fields
    }
