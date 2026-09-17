//! Keep interactive receipts valid and actionable when their content is large.
use serde_json::{json, Value};

pub(super) fn applies(name: &str) -> bool {
    matches!(
        name,
        "create_decision_board"
            | "read_decision_board"
            | "edit_decision_board"
            | "select_decision_card"
            | "inspect_prototype"
            | "set_prototype_control"
            | "undo_prototype_control"
    )
}

fn shorten(value: &mut Value, key: &str, limit: usize) -> bool {
    let Some(text) = value[key].as_str() else {
        return false;
    };
    if text.chars().count() <= limit {
        return false;
    }
    value[key] = json!(text.chars().take(limit).collect::<String>());
    true
}

fn count_cards(value: &Value) -> usize {
    value["columns"].as_array().map_or(0, |columns| {
        columns
            .iter()
            .map(|c| c["cards"].as_array().map_or(0, Vec::len))
            .sum()
    })
}

fn cursors(value: &mut Value) {
    if let Some(total) = value["total_cards"].as_u64() {
        let next = value["card_offset"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(count_cards(value) as u64);
        value["next_card_offset"] = if next < total {
            json!(next)
        } else {
            Value::Null
        };
    }
    if let Some(controls) = value["controls"].as_array_mut() {
        for control in controls.iter_mut() {
            let next = control["option_offset"]
                .as_u64()
                .unwrap_or(0)
                .saturating_add(control["options"].as_array().map_or(0, Vec::len) as u64);
            let total = control["total_options"].as_u64().unwrap_or(0);
            control["next_option_offset"] = if next < total {
                json!(next)
            } else {
                Value::Null
            };
        }
        let count = controls.len() as u64;
        let next = value["control_offset"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(count);
        let total = value["total_controls"].as_u64().unwrap_or(count);
        value["next_control_offset"] = if next < total {
            json!(next)
        } else {
            Value::Null
        };
    }
}

pub(super) fn render(text: &str, args: &Value, budget: usize) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return json!({"error":"Invalid structured interaction receipt"}).to_string();
    };
    if !value.is_object() {
        return json!({"error":"Invalid structured interaction receipt"}).to_string();
    }
    for (key, child) in [("columns", "cards"), ("controls", "options")] {
        if let Some(entries) = value.get(key) {
            let valid = entries.as_array().is_some_and(|items| {
                items.iter().all(|item| {
                    item.is_object()
                        && item.get(child).is_none_or(|children| {
                            children
                                .as_array()
                                .is_some_and(|items| items.iter().all(Value::is_object))
                        })
                })
            });
            if !valid {
                return json!({"error":"Malformed interaction receipt; inspect before retrying any edit"}).to_string();
            }
        }
    }
    if value["columns"].is_array() {
        let selected = args["card_id"].as_str();
        let offset = args["card_offset"].as_u64().unwrap_or(0);
        let total = count_cards(&value);
        let mut index = 0u64;
        for column in value["columns"].as_array_mut().unwrap() {
            if let Some(cards) = column["cards"].as_array_mut() {
                cards.retain(|card| {
                    let keep =
                        selected.map_or(index >= offset, |id| card["id"].as_str() == Some(id));
                    index += 1;
                    keep
                });
            }
        }
        value["card_offset"] = json!(if selected.is_some() { 0 } else { offset });
        value["total_cards"] = json!(if selected.is_some() {
            count_cards(&value)
        } else {
            total
        });
    }
    cursors(&mut value);
    if value.to_string().chars().count() <= budget {
        return value.to_string();
    }
    value["content_shortened"] = json!(true);
    // Only descriptive content is abbreviated. IDs, values, revisions, change
    // receipts and undo tokens are never clipped or reconstructed.
    value["note"] = json!("Some text is abbreviated. Read card_id for a specific card; use next_card_offset to continue. For controls use next_control_offset, or control_id and next_option_offset. Never guess omitted content.");
    value.as_object_mut().unwrap().remove("evidence");
    for limit in [256, 64, 16, 0] {
        shorten(&mut value, "output", limit);
        if let Some(columns) = value["columns"].as_array_mut() {
            for column in columns {
                if let Some(cards) = column["cards"].as_array_mut() {
                    for card in cards {
                        shorten(card, "body", limit);
                    }
                }
            }
        }
        if value.to_string().chars().count() <= budget {
            return value.to_string();
        }
    }
    loop {
        let mut removed = false;
        let remaining = count_cards(&value);
        if let Some(columns) = value["columns"].as_array_mut() {
            for column in columns.iter_mut().rev() {
                if let Some(cards) = column["cards"].as_array_mut() {
                    if remaining > 1 {
                        removed = cards.pop().is_some();
                        if removed {
                            break;
                        }
                    }
                }
            }
        }
        if !removed {
            if let Some(controls) = value["controls"].as_array_mut() {
                if controls.len() > 1 {
                    controls.pop();
                    removed = true;
                } else if let Some(control) = controls.first_mut() {
                    if let Some(options) = control["options"].as_array_mut() {
                        if options.len() > 1 {
                            options.pop();
                            removed = true;
                        }
                    }
                }
            }
        }
        cursors(&mut value);
        if value.to_string().chars().count() <= budget {
            return value.to_string();
        }
        if !removed {
            break;
        }
    }
    // A pathological single item or unusually small configured budget still
    // gets the authoritative receipt. Do not turn a completed edit into failure.
    let mut receipt = json!({"details_omitted":true,"reason":"Response budget too small for one item; inspect a narrower target or increase max_tool_chars. Do not repeat a completed edit."});
    for key in [
        "board_id",
        "revision",
        "selected_card_id",
        "undoable_changes",
        "prototype_id",
        "snapshot_id",
        "undo_id",
        "opened",
        "error",
    ] {
        if let Some(v) = value.get(key) {
            receipt[key] = v.clone();
        }
    }
    let text = receipt.to_string();
    if text.chars().count() <= budget {
        text
    } else {
        json!({"details_omitted":true,"reason":"Oversized interaction identity; no usable receipt. Inspect before retrying; do not assume an edit failed."}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_preview_entries_are_refused_without_panicking() {
        for value in [
            json!({"controls":["not a control"]}),
            json!({"controls":[{"options":[false]}]}),
            json!({"columns":[7]}),
        ] {
            let text = render(&value.to_string(), &json!({}), 1000);
            let result: Value = serde_json::from_str(&text).unwrap();
            assert!(result.get("error").is_some());
        }
    }

    #[test]
    fn long_board_keeps_revision_selection_and_undo() {
        let cards: Vec<_> = (0..4)
            .map(|i| json!({"id":format!("card-{i}"),"title":"Idea","body":"x".repeat(3000)}))
            .collect();
        let board = json!({"board_id":"board","revision":7,"selected_card_id":"card-3","undoable_changes":[6,7],"columns":[{"id":"now","title":"Now","cards":cards}]});
        let rendered = render(&board.to_string(), &json!({}), 12000);
        let result: Value = serde_json::from_str(&rendered).unwrap();
        assert!(rendered.chars().count() <= 12000);
        assert_eq!(result["revision"], 7);
        assert_eq!(result["selected_card_id"], "card-3");
        assert_eq!(result["undoable_changes"], json!([6, 7]));
        assert_eq!(count_cards(&result), 4);
        assert_eq!(result["content_shortened"], true);
        let focused: Value = serde_json::from_str(&render(
            &board.to_string(),
            &json!({"card_id":"card-3"}),
            12000,
        ))
        .unwrap();
        assert_eq!(count_cards(&focused), 1);
        assert_eq!(focused["columns"][0]["cards"][0]["body"], "x".repeat(3000));
    }

    #[test]
    fn board_pages_do_not_lose_or_duplicate_cards() {
        let cards: Vec<_> = (0..64).map(|i| json!({"id":format!("card-{i}"),"title":"名".repeat(60),"body":"\\\"".repeat(1500)})).collect();
        let board = json!({"board_id":"board","revision":3,"columns":[{"id":"now","title":"Now","cards":cards}]});
        let mut offset = 0;
        let mut seen = Vec::new();
        loop {
            let text = render(&board.to_string(), &json!({"card_offset":offset}), 2000);
            assert!(text.chars().count() <= 2000);
            let page: Value = serde_json::from_str(&text).unwrap();
            seen.extend(
                page["columns"][0]["cards"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c["id"].as_str().unwrap().to_owned()),
            );
            let Some(next) = page["next_card_offset"].as_u64() else {
                break;
            };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(
            seen,
            (0..64).map(|i| format!("card-{i}")).collect::<Vec<_>>()
        );
    }

    #[test]
    fn small_budget_retains_receipt_without_invalid_json() {
        let columns: Vec<_> = (0..8)
            .map(|i| json!({"id":format!("column-{i}"),"title":"x".repeat(100),"cards":[]}))
            .collect();
        let state = json!({"board_id":"board","revision":8,"selected_card_id":"card","undoable_changes":[7,8],"columns":columns});
        let text = render(&state.to_string(), &json!({}), 1000);
        assert!(text.chars().count() <= 1000);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["revision"], 8);
        assert_eq!(result["undoable_changes"], json!([7, 8]));
        assert_eq!(result["details_omitted"], true);
    }

    #[test]
    fn large_dropdown_preserves_exact_tokens_and_pages_options() {
        let options: Vec<_> = (0..64)
            .map(|i| json!({"value":format!("value-{i}"),"label":"x".repeat(180)}))
            .collect();
        let state = json!({"prototype_id":"proto","snapshot_id":"snapshot:2","undo_id":"undo","total_controls":1,"control_offset":0,"controls":[{"control_id":"control-1","value":"value-2","total_options":64,"option_offset":0,"options":options}],"changed":{"control_id":"control-1","after":"value-2"}});
        let text = render(&state.to_string(), &json!({}), 12000);
        assert!(text.chars().count() <= 12000);
        let result: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result["snapshot_id"], "snapshot:2");
        assert_eq!(result["prototype_id"], "proto");
        assert_eq!(result["undo_id"], "undo");
        assert_eq!(result["changed"], state["changed"]);
        assert!(
            result["controls"][0]["next_option_offset"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
}
