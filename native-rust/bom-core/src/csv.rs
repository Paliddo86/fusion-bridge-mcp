use crate::{Bom, Error, Metadata, Result, MAX_CSV_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsvMode {
    Flat,
    Structured,
}

enum Cell<'a> {
    Text(&'a str),
    Number(String),
    Boolean(bool),
}

/// Produce bounded UTF-8 RFC-4180-style CSV bytes with CRLF records. No file is
/// opened or published. Destination checks/atomic publication belong to the
/// future Rust runtime adapter.
pub fn csv_bytes(bom: &Bom, mode: CsvMode) -> Result<Vec<u8>> {
    if !bom.complete || !bom.configuration.graph_complete {
        return Err(Error::new(
            "source_incomplete",
            "Cannot export an incomplete BOM",
        ));
    }
    let mut output = Vec::new();
    let mut keys = match mode {
        CsvMode::Flat => vec![
            "componentId",
            "partNumber",
            "name",
            "description",
            "quantity",
            "supplier",
            "supplierPartNumber",
            "sourceUrl",
            "sha256",
            "unitMass",
            "unitVolume",
            "scope",
        ],
        CsvMode::Structured => vec![
            "path",
            "parentPath",
            "depth",
            "componentId",
            "name",
            "partNumber",
            "description",
            "quantity",
            "bodyCount",
            "childCount",
            "asPurchased",
            "supplier",
            "supplierPartNumber",
            "sourceUrl",
            "sha256",
        ],
    };
    keys.extend([
        "documentId",
        "revision",
        "configurationTableId",
        "configurationRowId",
        "configurationRowName",
    ]);
    record(&mut output, keys.into_iter().map(Cell::Text).collect())?;
    let context = || {
        vec![
            Cell::Text(&bom.target.document_id),
            Cell::Text(&bom.target.revision),
            Cell::Text(bom.configuration.table_id.as_deref().unwrap_or("")),
            Cell::Text(bom.configuration.row_id.as_deref().unwrap_or("")),
            Cell::Text(bom.configuration.row_name.as_deref().unwrap_or("")),
        ]
    };
    fn supplier(metadata: &Metadata) -> Vec<Cell<'_>> {
        vec![
            Cell::Text(metadata.supplier.as_deref().unwrap_or("")),
            Cell::Text(metadata.supplier_part_number.as_deref().unwrap_or("")),
            Cell::Text(metadata.source_url.as_deref().unwrap_or("")),
            Cell::Text(metadata.sha256.as_deref().unwrap_or("")),
        ]
    }
    match mode {
        CsvMode::Flat => {
            for row in &bom.parts {
                if !row.unit_mass.is_finite()
                    || !row.unit_volume.is_finite()
                    || row.unit_mass < 0.0
                    || row.unit_volume < 0.0
                {
                    return Err(Error::new(
                        "invalid_physical_properties",
                        "CSV mass/volume must be finite and nonnegative",
                    ));
                }
                let mut cells = vec![
                    Cell::Text(&row.component_id),
                    Cell::Text(&row.metadata.part_number),
                    Cell::Text(&row.metadata.name),
                    Cell::Text(&row.metadata.description),
                    Cell::Number(row.quantity.to_string()),
                ];
                cells.extend(supplier(&row.metadata));
                cells.extend([
                    Cell::Number(serde_json::to_string(&row.unit_mass).expect("finite float")),
                    Cell::Number(serde_json::to_string(&row.unit_volume).expect("finite float")),
                    Cell::Text(&row.scope),
                ]);
                cells.extend(context());
                record(&mut output, cells)?;
            }
        }
        CsvMode::Structured => {
            for row in &bom.instances {
                let mut cells = vec![
                    Cell::Text(&row.path),
                    Cell::Text(row.parent_path.as_deref().unwrap_or("")),
                    Cell::Number(row.depth.to_string()),
                    Cell::Text(&row.component_id),
                    Cell::Text(&row.metadata.name),
                    Cell::Text(&row.metadata.part_number),
                    Cell::Text(&row.metadata.description),
                    Cell::Number("1".into()),
                    Cell::Number(row.body_count.to_string()),
                    Cell::Number(row.child_count.to_string()),
                    Cell::Boolean(row.as_purchased),
                ];
                cells.extend(supplier(&row.metadata));
                cells.extend(context());
                record(&mut output, cells)?;
            }
        }
    }
    Ok(output)
}

fn record(output: &mut Vec<u8>, cells: Vec<Cell<'_>>) -> Result<()> {
    for (index, cell) in cells.into_iter().enumerate() {
        if index > 0 {
            append(output, b",")?;
        }
        match cell {
            Cell::Text(value) => {
                // Legacy prefixes '=+-@'. Also neutralize whitespace/BOM
                // bypasses and leading tab/newline spreadsheet control cells.
                let trimmed = value.trim_start_matches(|character: char| {
                    character.is_whitespace() || character == '\u{feff}'
                });
                let formula = trimmed.starts_with(['=', '+', '-', '@'])
                    || value.starts_with(char::is_control);
                field(output, value, formula)?;
            }
            Cell::Number(value) => field(output, &value, false)?,
            Cell::Boolean(value) => field(output, if value { "True" } else { "False" }, false)?,
        }
    }
    append(output, b"\r\n")
}

fn field(output: &mut Vec<u8>, value: &str, prefix_apostrophe: bool) -> Result<()> {
    let quote = value
        .bytes()
        .any(|byte| [b',', b'"', b'\r', b'\n'].contains(&byte));
    if quote {
        append(output, b"\"")?;
    }
    if prefix_apostrophe {
        append(output, b"'")?;
    }
    let mut remainder = value;
    while let Some(index) = remainder.find('"') {
        append(output, &remainder.as_bytes()[..index])?;
        append(output, b"\"\"")?;
        remainder = &remainder[index + 1..];
    }
    append(output, remainder.as_bytes())?;
    if quote {
        append(output, b"\"")?;
    }
    Ok(())
}

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_CSV_BYTES.saturating_sub(output.len()) {
        return Err(Error::new("bom_budget", "BOM CSV exceeds its byte budget"));
    }
    output.extend_from_slice(bytes);
    Ok(())
}
