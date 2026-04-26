//! Shared banner rendering functionality

pub const BANNER: &str = r#"                                                     
  ██████   ██████   ████████    ██████  ████████  █████ ████
 ███░░███ ░░░░░███ ░░███░░███  ███░░███░░███░░███░░███ ░███ 
░███ ░░░   ███████  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███ 
░███  ███ ███░░███  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███ 
░░██████ ░░████████ ████ █████░░██████  ░███████  ░░███████ 
 ░░░░░░   ░░░░░░░░ ░░░░ ░░░░░  ░░░░░░   ░███░░░    ░░░░░███ 
                                        ░███       ███ ░███ 
                                        █████     ░░██████  
                                       ░░░░░       ░░░░░░   
"#;

/// Print the banner with gradient colors and custom title
pub fn print_banner_with_gradient(title: &str) {
    let lines: Vec<&str> = BANNER.lines().collect();

    // Define gradient colors (light green to dark green)
    let colors = [
        "\x1b[38;2;157;207;161m", // #9DCFA1 — light green
        "\x1b[38;2;132;190;137m", // #84BE89 — medium light green
        "\x1b[38;2;108;174;113m", // #6CAE71 — medium green
        "\x1b[38;2;85;157;90m",   // #559D5A — medium dark green
        "\x1b[38;2;63;141;68m",   // #3F8D44 — dark green
        "\x1b[38;2;43;122;48m",   // #2B7A30 — darker green
        "\x1b[38;2;26;102;32m",   // #1A6620 — deep green
        "\x1b[38;2;12;82;18m",    // #0C5212 — deepest forest
    ];

    // Print each line with a different color from the gradient
    for (i, line) in lines.iter().enumerate() {
        let color_index =
            (i as f32 / lines.len() as f32 * (colors.len() - 1) as f32).round() as usize;
        let color = colors[color_index.min(colors.len() - 1)];
        println!("{}{}\x1b[0m", color, line);
    }

    // Print additional text in light green with custom title
    println!("\x1b[38;2;100;255;100m  \x1b[1m{}\x1b[0m", title);
    println!("  ─────────────────────────────────────────────");
    println!();
}

/// Print the banner with a single color (original behavior)
#[allow(dead_code)]
pub fn print_banner_single_color() {
    println!("\x1b[32m{BANNER}\x1b[0m");
    println!("  \x1b[1mAgent Hub — Setup Wizard\x1b[0m");
    println!("  ─────────────────────────────────────────────");
    println!();
}
