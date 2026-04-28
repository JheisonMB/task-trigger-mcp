//! Shared banner rendering functionality

pub const BANNER_GRADIENT: [(u8, u8, u8); 8] = [
    (157, 207, 161),
    (132, 190, 137),
    (108, 174, 113),
    (85, 157, 90),
    (63, 141, 68),
    (43, 122, 48),
    (26, 102, 32),
    (12, 82, 18),
];

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

    // Print each line with a different color from the gradient
    for (i, line) in lines.iter().enumerate() {
        let (r, g, b) = gradient_rgb(i, lines.len());
        println!("\x1b[38;2;{r};{g};{b}m{line}\x1b[0m");
    }

    // Print additional text in light green with custom title
    println!("\x1b[38;2;100;255;100m  \x1b[1m{}\x1b[0m", title);
    println!("  ─────────────────────────────────────────────");
    println!();
}

pub fn gradient_rgb(index: usize, line_count: usize) -> (u8, u8, u8) {
    let denominator = line_count.max(1);
    let color_index =
        (index as f32 / denominator as f32 * (BANNER_GRADIENT.len() - 1) as f32).round() as usize;
    BANNER_GRADIENT[color_index.min(BANNER_GRADIENT.len() - 1)]
}

/// Print the banner with a single color (original behavior)
#[allow(dead_code)]
pub fn print_banner_single_color() {
    println!("\x1b[32m{BANNER}\x1b[0m");
    println!("  \x1b[1mAgent Hub — Setup Wizard\x1b[0m");
    println!("  ─────────────────────────────────────────────");
    println!();
}
