use crate::render::scene::Fill;
use tiny_skia::Color;

/// Parses a basic CSS named color (`black`, `white`, `red`, `green`, `blue`, `yellow`, `cyan`,
/// `magenta`, `gray`, `orange`, `purple`, `pink`, `brown`, `navy`, `teal`, `olive`, `maroon`,
/// `silver`, `lime`, `transparent`, `none`) into a [`Color`].
pub fn parse_named_color(value: &str) -> Option<Color> {
    let rgb = match value.to_ascii_lowercase().as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "cyan" | "aqua" => (0, 255, 255),
        "magenta" | "fuchsia" => (255, 0, 255),
        "gray" | "grey" => (128, 128, 128),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "pink" => (255, 192, 203),
        "brown" => (165, 42, 42),
        "navy" => (0, 0, 128),
        "teal" => (0, 128, 128),
        "olive" => (128, 128, 0),
        "maroon" => (128, 0, 0),
        "silver" => (192, 192, 192),
        "lime" => (0, 255, 0),
        "transparent" | "none" => return Some(Color::from_rgba8(0, 0, 0, 0)),
        _ => return None,
    };
    Some(Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255))
}

fn hsl_to_rgb(h_deg: f32, s: f32, l: f32, a: f32) -> Color {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);
    let a = a.clamp(0.0, 1.0);
    let h = h_deg.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let (r1, g1, b1) = if (0.0..1.0).contains(&h_prime) {
        (c, x, 0.0)
    } else if (1.0..2.0).contains(&h_prime) {
        (x, c, 0.0)
    } else if (2.0..3.0).contains(&h_prime) {
        (0.0, c, x)
    } else if (3.0..4.0).contains(&h_prime) {
        (0.0, x, c)
    } else if (4.0..5.0).contains(&h_prime) {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    let m = l - c / 2.0;
    let r = ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let g = ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let b = ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    let alpha = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_rgba8(r, g, b, alpha)
}

fn parse_hsl(trimmed: &str) -> Option<Color> {
    let is_hsl = trimmed.starts_with("hsl(") && trimmed.ends_with(')');
    let is_hsla = trimmed.starts_with("hsla(") && trimmed.ends_with(')');
    if !is_hsl && !is_hsla {
        return None;
    }
    let prefix_len = if is_hsl { 4 } else { 5 };
    let inside = &trimmed[prefix_len..trimmed.len() - 1];
    let norm = inside.replace('/', ",");
    let parts: Vec<&str> = if norm.contains(',') {
        norm.split(',').map(str::trim).collect()
    } else {
        norm.split_whitespace().collect()
    };

    if parts.len() < 3 {
        return None;
    }

    let h_str = parts[0].trim();
    let h: f32 = if let Some(deg) = h_str.strip_suffix("deg") {
        deg.trim().parse().ok()?
    } else if let Some(rad) = h_str.strip_suffix("rad") {
        rad.trim().parse::<f32>().ok()?.to_degrees()
    } else if let Some(turn) = h_str.strip_suffix("turn") {
        turn.trim().parse::<f32>().ok()? * 360.0
    } else {
        h_str.parse().ok()?
    };

    let s_str = parts[1].trim().strip_suffix('%')?;
    let s: f32 = s_str.trim().parse::<f32>().ok()? / 100.0;

    let l_str = parts[2].trim().strip_suffix('%')?;
    let l: f32 = l_str.trim().parse::<f32>().ok()? / 100.0;

    let a: f32 = if parts.len() >= 4 {
        let a_str = parts[3].trim();
        if let Some(pct) = a_str.strip_suffix('%') {
            (pct.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0)
        } else {
            a_str.parse::<f32>().ok()?.clamp(0.0, 1.0)
        }
    } else {
        1.0
    };

    Some(hsl_to_rgb(h, s, l, a))
}

pub fn parse_color(value: &str) -> Option<Color> {
    let trimmed = value.trim();
    if let Some(named) = parse_named_color(trimmed) {
        return Some(named);
    }

    if let Some(hex) = trimmed.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Color::from_rgba8(r, g, b, 255))
            }
            4 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                let a = u8::from_str_radix(&hex[3..4], 16).ok()? * 17;
                Some(Color::from_rgba8(r, g, b, a))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Color::from_rgba8(r, g, b, 255))
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Color::from_rgba8(r, g, b, a))
            }
            _ => None,
        };
    }

    if trimmed.starts_with("rgb(") && trimmed.ends_with(')') {
        let inside = &trimmed[4..trimmed.len() - 1];
        let parts: Vec<&str> = inside.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            let r = parts[0].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let g = parts[1].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let b = parts[2].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            return Some(Color::from_rgba8(r, g, b, 255));
        }
    }

    if trimmed.starts_with("rgba(") && trimmed.ends_with(')') {
        let inside = &trimmed[5..trimmed.len() - 1];
        let parts: Vec<&str> = inside.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r = parts[0].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let g = parts[1].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let b = parts[2].parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8;
            let a_str = parts[3].trim_end_matches('%');
            let a = if let Ok(val) = a_str.parse::<f32>() {
                if parts[3].ends_with('%') {
                    (val.clamp(0.0, 100.0) * 2.55).round() as u8
                } else if val <= 1.0 {
                    (val.clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    val.clamp(0.0, 255.0).round() as u8
                }
            } else {
                255
            };
            return Some(Color::from_rgba8(r, g, b, a));
        }
    }

    if let Some(hsl_col) = parse_hsl(trimmed) {
        return Some(hsl_col);
    }

    if trimmed.starts_with("linear-gradient") || trimmed.starts_with("radial-gradient") {
        if let Some(start) = trimmed.find("rgba(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("rgb(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("hsla(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find("hsl(") {
            if let Some(end) = trimmed[start..].find(')') {
                return parse_color(&trimmed[start..start + end + 1]);
            }
        } else if let Some(start) = trimmed.find('#') {
            let hex_cand = &trimmed[start..];
            let end = hex_cand
                .find(|c: char| !c.is_ascii_hexdigit() && c != '#')
                .unwrap_or(hex_cand.len());
            return parse_color(&hex_cand[..end]);
        }
    }

    None
}

fn split_gradient_args(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let tok = current.trim().to_string();
                if !tok.is_empty() {
                    tokens.push(tok);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let tok = current.trim().to_string();
    if !tok.is_empty() {
        tokens.push(tok);
    }
    tokens
}

fn parse_angle_or_direction(s: &str) -> Option<f32> {
    let lower = s.to_ascii_lowercase();
    let trimmed = lower.trim();
    if let Some(num_str) = trimmed.strip_suffix("deg") {
        return num_str.trim().parse::<f32>().ok();
    }
    if let Some(num_str) = trimmed.strip_suffix("rad") {
        return num_str.trim().parse::<f32>().ok().map(|r| r.to_degrees());
    }
    if let Some(num_str) = trimmed.strip_suffix("turn") {
        return num_str.trim().parse::<f32>().ok().map(|t| t * 360.0);
    }
    if let Some(dir) = trimmed.strip_prefix("to ") {
        let dir = dir.trim();
        return match dir {
            "top" => Some(0.0),
            "right" => Some(90.0),
            "bottom" => Some(180.0),
            "left" => Some(270.0),
            "top right" | "right top" => Some(45.0),
            "bottom right" | "right bottom" => Some(135.0),
            "bottom left" | "left bottom" => Some(225.0),
            "top left" | "left top" => Some(315.0),
            _ => None,
        };
    }
    None
}

fn parse_color_stop(s: &str) -> Option<(Option<f32>, Color)> {
    let trimmed = s.trim();
    let mut last_space = None;
    let mut depth = 0;
    for (idx, ch) in trimmed.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ' ' | '\t' if depth == 0 => last_space = Some(idx),
            _ => {}
        }
    }

    if let Some(idx) = last_space {
        let (color_part, offset_part) = trimmed.split_at(idx);
        let offset_trimmed = offset_part.trim();
        let offset_val = if let Some(pct) = offset_trimmed.strip_suffix('%') {
            pct.trim()
                .parse::<f32>()
                .ok()
                .map(|p| (p / 100.0).clamp(0.0, 1.0))
        } else {
            offset_trimmed
                .parse::<f32>()
                .ok()
                .map(|v| v.clamp(0.0, 1.0))
        };
        if let Some(color) = parse_color(color_part.trim()) {
            return Some((offset_val, color));
        }
    }

    if let Some(color) = parse_color(trimmed) {
        return Some((None, color));
    }

    None
}

pub fn parse_gradient(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if !trimmed.starts_with("linear-gradient(") || !trimmed.ends_with(')') {
        return None;
    }
    let inside = &trimmed[16..trimmed.len() - 1];
    let raw_args = split_gradient_args(inside);
    if raw_args.is_empty() {
        return None;
    }

    let mut angle_deg = 180.0;
    let mut color_args = &raw_args[..];

    if let Some(angle) = parse_angle_or_direction(&raw_args[0]) {
        angle_deg = angle;
        color_args = &raw_args[1..];
    }

    if color_args.len() < 2 {
        return None;
    }

    let mut parsed_stops = Vec::new();
    for arg in color_args {
        let (opt_offset, color) = parse_color_stop(arg)?;
        parsed_stops.push((opt_offset, color));
    }

    let total = parsed_stops.len();
    let mut final_stops = Vec::with_capacity(total);
    for (i, (opt_offset, color)) in parsed_stops.into_iter().enumerate() {
        let offset = opt_offset.unwrap_or_else(|| {
            if total <= 1 {
                0.0
            } else {
                i as f32 / (total - 1) as f32
            }
        });
        final_stops.push((offset, color));
    }

    Some(Fill::LinearGradient {
        angle_deg,
        stops: final_stops,
    })
}

pub fn parse_radial_gradient(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if !trimmed.starts_with("radial-gradient(") || !trimmed.ends_with(')') {
        return None;
    }
    let inside = &trimmed[16..trimmed.len() - 1];
    let raw_args = split_gradient_args(inside);
    if raw_args.is_empty() {
        return None;
    }

    let mut cx = 0.5;
    let mut cy = 0.5;
    let radius = 0.0;
    let mut color_args = &raw_args[..];

    let first = raw_args[0].trim().to_ascii_lowercase();
    if first.starts_with("circle") || first.contains("at ") {
        color_args = &raw_args[1..];
        if let Some(at_idx) = first.find("at ") {
            let pos_str = first[at_idx + 3..].trim();
            let pos_parts: Vec<&str> = pos_str.split_whitespace().collect();
            if pos_parts.len() == 1 {
                match pos_parts[0] {
                    "center" => {
                        cx = 0.5;
                        cy = 0.5;
                    }
                    "top" => {
                        cx = 0.5;
                        cy = 0.0;
                    }
                    "bottom" => {
                        cx = 0.5;
                        cy = 1.0;
                    }
                    "left" => {
                        cx = 0.0;
                        cy = 0.5;
                    }
                    "right" => {
                        cx = 1.0;
                        cy = 0.5;
                    }
                    _ => {}
                }
            } else if pos_parts.len() >= 2 {
                if let Some(x_pct) = pos_parts[0]
                    .strip_suffix('%')
                    .and_then(|p| p.parse::<f32>().ok())
                {
                    cx = (x_pct / 100.0).clamp(0.0, 1.0);
                }
                if let Some(y_pct) = pos_parts[1]
                    .strip_suffix('%')
                    .and_then(|p| p.parse::<f32>().ok())
                {
                    cy = (y_pct / 100.0).clamp(0.0, 1.0);
                }
            }
        }
    }

    if color_args.len() < 2 {
        return None;
    }

    let mut parsed_stops = Vec::new();
    for arg in color_args {
        let (opt_offset, color) = parse_color_stop(arg)?;
        parsed_stops.push((opt_offset, color));
    }

    let total = parsed_stops.len();
    let mut final_stops = Vec::with_capacity(total);
    for (i, (opt_offset, color)) in parsed_stops.into_iter().enumerate() {
        let offset = opt_offset.unwrap_or_else(|| {
            if total <= 1 {
                0.0
            } else {
                i as f32 / (total - 1) as f32
            }
        });
        final_stops.push((offset, color));
    }

    Some(Fill::RadialGradient {
        cx,
        cy,
        radius,
        stops: final_stops,
    })
}

pub fn parse_fill(value: &str) -> Option<Fill> {
    let trimmed = value.trim();
    if trimmed.starts_with("linear-gradient(") {
        if let Some(fill) = parse_gradient(trimmed) {
            return Some(fill);
        }
    }
    if trimmed.starts_with("radial-gradient(") {
        if let Some(fill) = parse_radial_gradient(trimmed) {
            return Some(fill);
        }
    }
    parse_color(trimmed).map(Fill::Solid)
}
