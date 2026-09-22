//! Generic Data-Binding & Template Resolution Engine.
//!
//! Provides scalar interpolation (`{module.battery.percent}%`),
//! list repetition (`repeat_over = "module.network.available_networks"`),
//! and ternary conditionals without knowledge of specific module names.

use crate::widgets::WidgetConfig;
use serde_json::Value;
use std::collections::HashMap;

/// Resolves a dot-separated path against item context and global module data store.
pub fn resolve_path<'a>(
    path: &str,
    context: Option<&'a Value>,
    data_store: &'a HashMap<String, Value>,
) -> Option<&'a Value> {
    let clean_path = path.trim();
    if clean_path.is_empty() {
        return None;
    }

    // 1. Check item context if path starts with "item." or "this."
    if let Some(subpath) = clean_path
        .strip_prefix("item.")
        .or_else(|| clean_path.strip_prefix("this."))
    {
        if let Some(ctx) = context {
            return traverse_json(ctx, subpath);
        }
        return None;
    }

    // 2. Check item context directly for local fields (e.g. "ssid", "signal")
    if let Some(ctx) = context {
        if let Some(val) = traverse_json(ctx, clean_path) {
            return Some(val);
        }
    }

    // 3. Module store lookup: "module.<mod_name>.<subpath>" or "<mod_name>.<subpath>"
    let lookup_path = clean_path.strip_prefix("module.").unwrap_or(clean_path);
    let mut parts = lookup_path.splitn(2, '.');
    let module_name = parts.next()?;
    let subpath = parts.next().unwrap_or("");

    if let Some(mod_val) = data_store.get(module_name) {
        if subpath.is_empty() {
            Some(mod_val)
        } else {
            traverse_json(mod_val, subpath)
        }
    } else {
        None
    }
}

/// Traverses a JSON value using dot notation and array index notation.
pub fn traverse_json<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = root;
    for part in path.split('.') {
        if part.is_empty() {
            continue;
        }
        if let Ok(idx) = part.parse::<usize>() {
            current = current.get(idx)?;
        } else {
            current = current.get(part)?;
        }
    }
    Some(current)
}

/// Evaluates a template string, replacing `{path}` or `{cond ? true_val : false_val}` placeholders.
pub fn eval_template(
    template: &str,
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> String {
    if !template.contains('{') || !template.contains('}') {
        return template.to_string();
    }

    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut expr = String::new();
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
                expr.push(inner);
            }
            if !found_close {
                result.push('{');
                result.push_str(&expr);
                break;
            }

            let expr_trimmed = expr.trim();

            // Check for ternary condition: "cond ? true_val : false_val"
            if let Some((cond_part, rest)) = expr_trimmed.split_once('?') {
                if let Some((true_val, false_val)) = rest.split_once(':') {
                    let is_truthy = is_condition_truthy(cond_part.trim(), context, data_store);
                    let selected = if is_truthy {
                        true_val.trim()
                    } else {
                        false_val.trim()
                    };
                    if (selected.starts_with('\'') && selected.ends_with('\''))
                        || (selected.starts_with('"') && selected.ends_with('"'))
                    {
                        let unquoted = &selected[1..selected.len() - 1];
                        result.push_str(unquoted);
                    } else if let Some(val) = resolve_path(selected, context, data_store) {
                        match val {
                            Value::String(s) => result.push_str(s),
                            Value::Number(n) => {
                                if let Some(i) = n.as_i64() {
                                    result.push_str(&i.to_string());
                                } else if let Some(f) = n.as_f64() {
                                    result.push_str(format!("{:.1}", f).trim_end_matches(".0"));
                                }
                            }
                            Value::Bool(b) => result.push_str(if *b { "true" } else { "false" }),
                            _ => result.push_str(&val.to_string()),
                        }
                    } else {
                        let unquoted = selected.trim_matches('\'').trim_matches('"');
                        result.push_str(unquoted);
                    }
                    continue;
                }
            }

            // Standard path resolution
            if let Some(val) = resolve_path(expr_trimmed, context, data_store) {
                match val {
                    Value::String(s) => result.push_str(s),
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            result.push_str(&i.to_string());
                        } else if let Some(f) = n.as_f64() {
                            result.push_str(format!("{:.1}", f).trim_end_matches(".0"));
                        }
                    }
                    Value::Bool(b) => result.push_str(if *b { "true" } else { "false" }),
                    Value::Null => {}
                    _ => result.push_str(&val.to_string()),
                }
            } else if expr_trimmed == "value" {
                // Preserve slider runtime placeholder {value} if not in data_store/context
                result.push('{');
                result.push_str(expr_trimmed);
                result.push('}');
            } else {
                // If path unresolved, leave empty or as is
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Checks if a condition expression evaluates to truthy.
fn is_condition_truthy(
    expr: &str,
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> bool {
    let (is_inverted, path) = if let Some(stripped) = expr.strip_prefix('!') {
        (true, stripped.trim())
    } else {
        (false, expr)
    };

    let truthy = match resolve_path(path, context, data_store) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|num| num != 0.0),
        Some(Value::String(s)) => !s.is_empty() && s != "false" && s != "0" && s != "off",
        Some(Value::Array(arr)) => !arr.is_empty(),
        Some(Value::Object(obj)) => !obj.is_empty(),
        Some(Value::Null) | None => false,
    };

    if is_inverted {
        !truthy
    } else {
        truthy
    }
}

/// Evaluates a numeric template expression (e.g. for sliders or progress rings).
pub fn eval_template_number(
    expr: &str,
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> Option<f32> {
    let clean = expr.trim().trim_matches('{').trim_matches('}');
    if let Some(val) = resolve_path(clean, context, data_store) {
        if let Some(num) = val.as_f64() {
            return Some(num as f32);
        }
        if let Some(s) = val.as_str() {
            if let Ok(num) = s.parse::<f32>() {
                return Some(num);
            }
        }
    }
    None
}

/// Recursively expands widget configurations by evaluating data bindings and expanding `repeat_over` lists.
pub fn expand_widget_configs(
    configs: &[WidgetConfig],
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> Vec<WidgetConfig> {
    let mut expanded = Vec::new();

    for cfg in configs {
        // If this widget repeats over an array
        if let Some(ref repeat_path) = cfg.repeat_over {
            if let Some(array_val) = resolve_path(repeat_path, context, data_store) {
                if let Some(items) = array_val.as_array() {
                    for item in items {
                        let mut item_node = expand_single_widget(cfg, Some(item), data_store);
                        item_node.repeat_over = None; // Avoid repeating again
                        item_node.children =
                            expand_widget_configs(&cfg.children, Some(item), data_store);
                        expanded.push(item_node);
                    }
                    continue;
                }
            }
            // If repeat array is empty or not found, skip or create empty container
            continue;
        }

        let mut node_cfg = expand_single_widget(cfg, context, data_store);
        node_cfg.children = expand_widget_configs(&cfg.children, context, data_store);
        expanded.push(node_cfg);
    }

    expanded
}

/// Evaluates all templated string and numeric properties on a single widget config.
fn expand_single_widget(
    cfg: &WidgetConfig,
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> WidgetConfig {
    let mut out = cfg.clone();

    if let Some(ref t) = cfg.text {
        out.text = Some(eval_template(t, context, data_store));
    }
    if let Some(ref act) = cfg.on_click {
        out.on_click = Some(eval_template(act, context, data_store));
    }
    if let Some(ref r_act) = cfg.on_right_click {
        out.on_right_click = Some(eval_template(r_act, context, data_store));
    }
    if let Some(ref chg) = cfg.on_change {
        out.on_change = Some(eval_template(chg, context, data_store));
    }
    if let Some(ref tt) = cfg.tooltip {
        out.tooltip = Some(eval_template(tt, context, data_store));
    }
    if let Some(ref s) = cfg.style {
        out.style = Some(eval_template(s, context, data_store));
    }
    if let Some(ref id) = cfg.id {
        out.id = Some(eval_template(id, context, data_store));
    }
    if let Some(ref p) = cfg.path {
        out.path = Some(eval_template(p, context, data_store));
    }

    // Dynamic numeric bindings
    if let Some(ref t) = cfg.text {
        if (cfg.ty == "slider"
            || cfg.ty == "progress"
            || cfg.ty == "ring"
            || cfg.ty == "progress_ring")
            && cfg.value.is_none()
        {
            if let Some(num) = eval_template_number(t, context, data_store) {
                out.value = Some(num);
            }
        }
    }

    let effective_bind = cfg.bind.clone().or_else(|| {
        if cfg.ty == "slider" {
            if let Some(ref id) = cfg.id {
                if id.contains("volume") || id.contains("audio") {
                    return Some("audio.volume".to_string());
                } else if id.contains("brightness") {
                    return Some("brightness.percent".to_string());
                } else if id.contains("mic") {
                    return Some("audio.mic_volume".to_string());
                }
            }
        }
        None
    });

    if let Some(ref bind_path) = effective_bind {
        if let Some(data) = resolve_path(bind_path, context, data_store) {
            if let Some(ref fmt) = cfg.format {
                let mut local_store = data_store.clone();
                local_store.insert("value".to_string(), data.clone());
                out.text = Some(eval_template(fmt, Some(data), &local_store));
            } else if let Some(s) = data.as_str() {
                out.text = Some(s.to_string());
                if let Ok(num) = s.parse::<f32>() {
                    out.value = Some(num);
                }
            } else if let Some(num) = data.as_f64() {
                out.text = Some(num.to_string());
                out.value = Some(num as f32);
            }
        }
    }

    out
}

/// Safely evaluates basic arithmetic expressions (+, -, *, /).
pub fn evaluate_math_expression(expr: &str) -> Option<f32> {
    let clean = expr.trim().replace(' ', "");
    if clean.is_empty() {
        return None;
    }
    if let Ok(val) = clean.parse::<f32>() {
        return Some(val);
    }

    if clean.starts_with('(') && clean.ends_with(')') {
        let mut depth = 0;
        let mut full_enclosed = true;
        for (i, c) in clean.chars().enumerate() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 && i < clean.len() - 1 {
                    full_enclosed = false;
                    break;
                }
            }
        }
        if full_enclosed && depth == 0 {
            return evaluate_math_expression(&clean[1..clean.len() - 1]);
        }
    }

    let mut depth = 0;
    let mut last_add_sub = None;
    for (i, c) in clean.chars().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '+' | '-' if depth == 0 && i > 0 => {
                last_add_sub = Some((i, c));
            }
            _ => {}
        }
    }
    if let Some((idx, op)) = last_add_sub {
        let left = evaluate_math_expression(&clean[..idx])?;
        let right = evaluate_math_expression(&clean[idx + 1..])?;
        return match op {
            '+' => Some(left + right),
            '-' => Some(left - right),
            _ => None,
        };
    }

    let mut depth = 0;
    let mut last_mul_div = None;
    for (i, c) in clean.chars().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '*' | '/' if depth == 0 && i > 0 => {
                last_mul_div = Some((i, c));
            }
            _ => {}
        }
    }
    if let Some((idx, op)) = last_mul_div {
        let left = evaluate_math_expression(&clean[..idx])?;
        let right = evaluate_math_expression(&clean[idx + 1..])?;
        return match op {
            '*' => Some(left * right),
            '/' => {
                if right.abs() < 1e-6 {
                    None
                } else {
                    Some(left / right)
                }
            }
            _ => None,
        };
    }

    None
}

/// Evaluates mathematical expressions with data binding interpolation.
pub fn eval_position(
    expr: &str,
    context: Option<&Value>,
    data_store: &HashMap<String, Value>,
) -> Option<f32> {
    let templated = eval_template(expr, context, data_store);
    evaluate_math_expression(&templated)
}

/// Binds live module data directly to a widget node in the tree.
pub fn bind_widget(
    widget: &mut crate::widgets::WidgetNode,
    bind_path: &str,
    data_store: &HashMap<String, Value>,
) -> bool {
    if let Some(data) = resolve_path(bind_path, None, data_store) {
        match &mut widget.content {
            crate::widgets::WidgetContent::Text { text } => {
                if let Some(format) = widget.layout.format.as_ref() {
                    let mut local_store = data_store.clone();
                    local_store.insert("value".to_string(), data.clone());
                    *text = eval_template(format, Some(data), &local_store);
                } else if let Some(s) = data.as_str() {
                    *text = s.to_string();
                } else if let Some(num) = data.as_f64() {
                    *text = num.to_string();
                }
                return true;
            }
            crate::widgets::WidgetContent::Button { label, .. } => {
                if let Some(format) = widget.layout.format.as_ref() {
                    let mut local_store = data_store.clone();
                    local_store.insert("value".to_string(), data.clone());
                    *label = eval_template(format, Some(data), &local_store);
                } else if let Some(s) = data.as_str() {
                    *label = s.to_string();
                }
                return true;
            }
            crate::widgets::WidgetContent::Progress { value, .. }
            | crate::widgets::WidgetContent::Slider { value, .. } => {
                if let Some(num) = data.as_f64() {
                    *value = num as f32;
                    return true;
                }
            }
            crate::widgets::WidgetContent::ProgressRing { value, .. } => {
                if let Some(num) = data.as_f64() {
                    *value = num as f32;
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_scalar_binding_interpolation() {
        let mut store = HashMap::new();
        store.insert(
            "battery".to_string(),
            json!({ "percent": 84, "charging": true }),
        );
        store.insert(
            "network".to_string(),
            json!({ "connected_ssid": "MyWiFi", "signal": 95 }),
        );

        let res = eval_template(
            "Battery: {module.battery.percent}% on {network.connected_ssid}",
            None,
            &store,
        );
        assert_eq!(res, "Battery: 84% on MyWiFi");
    }

    #[test]
    fn test_ternary_conditional_interpolation() {
        let mut store = HashMap::new();
        store.insert("network".to_string(), json!({ "connected": true }));

        let res = eval_template(
            "{module.network.connected ? 'chip_accent' : 'chip'}",
            None,
            &store,
        );
        assert_eq!(res, "chip_accent");

        store.insert("network".to_string(), json!({ "connected": false }));
        let res2 = eval_template(
            "{module.network.connected ? 'chip_accent' : 'chip'}",
            None,
            &store,
        );
        assert_eq!(res2, "chip");
    }

    #[test]
    fn test_repeat_over_expansion() {
        let mut store = HashMap::new();
        store.insert(
            "network".to_string(),
            json!({
                "available_networks": [
                    { "ssid": "HomeWiFi", "signal": 90, "connected": true },
                    { "ssid": "OfficeWiFi", "signal": 60, "connected": false }
                ]
            }),
        );

        let template_cfg = vec![WidgetConfig {
            ty: "container".to_string(),
            style: Some("card".to_string()),
            repeat_over: Some("module.network.available_networks".to_string()),
            children: vec![WidgetConfig {
                ty: "button".to_string(),
                id: Some("net_{item.ssid}".to_string()),
                text: Some("{item.ssid} ({item.signal}%)".to_string()),
                style: Some("{item.connected ? 'chip_accent' : 'chip'}".to_string()),
                on_click: Some("connect {item.ssid}".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }];

        let expanded = expand_widget_configs(&template_cfg, None, &store);
        assert_eq!(expanded.len(), 2);
        assert_eq!(
            expanded[0].children[0].text.as_deref(),
            Some("HomeWiFi (90%)")
        );
        assert_eq!(
            expanded[0].children[0].style.as_deref(),
            Some("chip_accent")
        );
        assert_eq!(
            expanded[0].children[0].on_click.as_deref(),
            Some("connect HomeWiFi")
        );

        assert_eq!(
            expanded[1].children[0].text.as_deref(),
            Some("OfficeWiFi (60%)")
        );
        assert_eq!(expanded[1].children[0].style.as_deref(), Some("chip"));
        assert_eq!(
            expanded[1].children[0].on_click.as_deref(),
            Some("connect OfficeWiFi")
        );
    }

    #[test]
    fn test_math_eval_and_position() {
        assert_eq!(evaluate_math_expression("100 + 50 * 2"), Some(200.0));
        assert_eq!(evaluate_math_expression("40 + (3 - 1) * 25"), Some(90.0));
        assert_eq!(evaluate_math_expression("300 - 150 / 3"), Some(250.0));

        let mut store = HashMap::new();
        store.insert("audio".to_string(), json!({ "volume": 50 }));
        let pos = eval_position("100 + {module.audio.volume} * 2", None, &store);
        assert_eq!(pos, Some(200.0));
    }

    #[test]
    fn test_bind_widget_reactive() {
        let mut store = HashMap::new();
        store.insert("audio".to_string(), json!({ "volume": 75 }));

        let mut node = crate::widgets::WidgetNode::new(crate::widgets::WidgetContent::Text {
            text: String::new(),
        });
        node.layout.format = Some("Volume: {value}%".to_string());

        assert!(bind_widget(&mut node, "module.audio.volume", &store));
        if let crate::widgets::WidgetContent::Text { text } = &node.content {
            assert_eq!(text, "Volume: 75%");
        } else {
            panic!("Expected text node");
        }
    }

    #[test]
    fn test_eval_template_preserves_slider_value() {
        let store = HashMap::new();
        let tmpl = "event:audio:volume_slider:{value}";
        let res = eval_template(tmpl, None, &store);
        assert_eq!(res, "event:audio:volume_slider:{value}");
    }

    #[test]
    fn test_slider_bind_inference_and_out_value() {
        let mut store = HashMap::new();
        store.insert("audio".to_string(), json!({ "volume": 85 }));

        let slider_cfg = WidgetConfig {
            ty: "slider".to_string(),
            id: Some("qs_volume_slider".to_string()),
            on_change: Some("event:audio:volume_slider:{value}".to_string()),
            min: Some(0.0),
            max: Some(100.0),
            value: Some(50.0), // initial dummy value in config
            ..Default::default()
        };

        let expanded = expand_single_widget(&slider_cfg, None, &store);
        assert_eq!(expanded.value, Some(85.0));
        assert_eq!(
            expanded.on_change.as_deref(),
            Some("event:audio:volume_slider:{value}")
        );
    }

    #[test]
    fn test_eval_template_ternary_variable_resolution() {
        let mut store = HashMap::new();
        store.insert(
            "mpris".to_string(),
            json!({
                "title": "Starboy",
                "artist": "The Weeknd"
            }),
        );

        let tmpl_playing = "{mpris.title ? mpris.title : 'No media playing'}";
        let res1 = eval_template(tmpl_playing, None, &store);
        assert_eq!(res1, "Starboy");

        let store_empty = HashMap::new();
        let res2 = eval_template(tmpl_playing, None, &store_empty);
        assert_eq!(res2, "No media playing");
    }
}
