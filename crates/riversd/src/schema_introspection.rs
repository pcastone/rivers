//! Schema introspection — validates DataView fields against database columns at startup.

/// A single schema mismatch found during introspection.
#[derive(Debug)]
pub struct SchemaMismatch {
    /// Name of the DataView with the mismatch.
    pub dataview_name: String,
    /// Schema field name that was not found in query results.
    pub field_name: String,
    /// Actual columns returned by the query.
    pub available_columns: Vec<String>,
    /// Levenshtein suggestion if a close match exists.
    pub suggestion: Option<String>,
}

impl std::fmt::Display for SchemaMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DataView '{}' field '{}' not found — available: {}",
            self.dataview_name,
            self.field_name,
            self.available_columns.join(", ")
        )?;
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " — did you mean '{}'?", suggestion)?;
        }
        Ok(())
    }
}

/// Compare schema field names against actual query column names.
pub fn check_fields_against_columns(
    dataview_name: &str,
    schema_fields: &[String],
    actual_columns: &[String],
) -> Vec<SchemaMismatch> {
    let mut mismatches = Vec::new();
    for field in schema_fields {
        if !actual_columns.iter().any(|c| c == field) {
            let suggestion = suggest_column(field, actual_columns);
            mismatches.push(SchemaMismatch {
                dataview_name: dataview_name.to_string(),
                field_name: field.clone(),
                available_columns: actual_columns.to_vec(),
                suggestion,
            });
        }
    }
    mismatches
}

/// Suggest a column name using Levenshtein distance (max distance 2).
fn suggest_column(unknown: &str, columns: &[String]) -> Option<String> {
    let mut best: Option<(&str, usize)> = None;
    for col in columns {
        let dist = levenshtein(unknown, col);
        if dist <= 2 {
            if best.is_none() || dist < best.unwrap().1 {
                best = Some((col, dist));
            }
        }
    }
    best.map(|(s, _)| s.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut matrix = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() {
        matrix[i][0] = i;
    }
    for j in 0..=b.len() {
        matrix[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }
    matrix[a.len()][b.len()]
}

/// Format all mismatches into a single error message for startup failure.
pub fn format_introspection_errors(mismatches: &[SchemaMismatch]) -> String {
    if mismatches.len() == 1 {
        format!("schema introspection failed: {}", mismatches[0])
    } else {
        let details: Vec<String> = mismatches.iter().map(|m| format!("  {}", m)).collect();
        format!(
            "schema introspection failed — {} mismatches found:\n{}",
            mismatches.len(),
            details.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_fields_match() {
        let fields = vec!["id".into(), "name".into(), "qty".into()];
        let columns = vec!["id".into(), "name".into(), "qty".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert!(mismatches.is_empty());
    }

    #[test]
    fn one_field_missing_with_suggestion() {
        let fields = vec!["id".into(), "namee".into()];
        let columns = vec!["id".into(), "name".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 1);
        assert_eq!(mismatches[0].field_name, "namee");
        assert_eq!(mismatches[0].suggestion, Some("name".to_string()));
    }

    #[test]
    fn no_suggestion_for_distant_field() {
        let fields = vec!["zzzzz".into()];
        let columns = vec!["id".into(), "name".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 1);
        assert!(mismatches[0].suggestion.is_none());
    }

    #[test]
    fn multiple_mismatches_collected() {
        let fields = vec!["idd".into(), "namee".into(), "qtyz".into()];
        let columns = vec!["id".into(), "name".into(), "qty".into()];
        let mismatches = check_fields_against_columns("dv", &fields, &columns);
        assert_eq!(mismatches.len(), 3);
    }

    #[test]
    fn error_format_single() {
        let mismatches = vec![SchemaMismatch {
            dataview_name: "orders".into(),
            field_name: "qtyz".into(),
            available_columns: vec!["id".into(), "qty".into()],
            suggestion: Some("qty".into()),
        }];
        let msg = format_introspection_errors(&mismatches);
        assert!(msg.contains("orders"));
        assert!(msg.contains("qtyz"));
        assert!(msg.contains("qty"));
    }

    #[test]
    fn error_format_multiple() {
        let mismatches = vec![
            SchemaMismatch {
                dataview_name: "orders".into(),
                field_name: "qtyz".into(),
                available_columns: vec!["qty".into()],
                suggestion: Some("qty".into()),
            },
            SchemaMismatch {
                dataview_name: "orders".into(),
                field_name: "idd".into(),
                available_columns: vec!["id".into()],
                suggestion: Some("id".into()),
            },
        ];
        let msg = format_introspection_errors(&mismatches);
        assert!(msg.contains("2 mismatches"));
    }
}
