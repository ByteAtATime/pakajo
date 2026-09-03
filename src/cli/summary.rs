use crate::{
    color,
    events::{SummaryPackage, TransactionSummary},
    utils::format_mib,
};

pub fn print_summary(summary: &TransactionSummary) {
    if summary.packages.is_empty() {
        println!(" nothing to do");
        return;
    }
    print!("{}", render_summary(summary, color::stdout_color()));
}

struct SummaryRow {
    name: String,
    old_version: String,
    new_version: String,
    net_bytes: i64,
    download_bytes: i64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Align {
    Left,
    Right,
}

struct SummaryColumn {
    header: String,
    align: Align,
    cells: Vec<String>,
}

fn summary_rows(summary: &TransactionSummary) -> Vec<SummaryRow> {
    let mut ordered: Vec<&SummaryPackage> = summary.packages.iter().collect();
    ordered.sort_by_key(|p| (!p.is_removal, p.name.clone()));
    ordered
        .iter()
        .map(|p| {
            let name = match &p.repository {
                Some(repo) => format!("{repo}/{}", p.name),
                None => p.name.clone(),
            };
            let net_bytes = if p.is_removal {
                -p.installed_size
            } else {
                p.installed_size - p.old_installed_size
            };
            SummaryRow {
                name,
                old_version: p.old_version.clone().unwrap_or_default(),
                new_version: p.new_version.clone(),
                net_bytes,
                download_bytes: p.download_size,
            }
        })
        .collect()
}

fn build_columns(rows: &[SummaryRow], count: usize) -> Vec<SummaryColumn> {
    let has_old = rows.iter().any(|r| !r.old_version.is_empty());
    let has_new = rows.iter().any(|r| !r.new_version.is_empty());
    let has_dl = rows.iter().any(|r| r.download_bytes > 0);

    let mut columns: Vec<SummaryColumn> = Vec::new();
    columns.push(SummaryColumn {
        header: format!("Package ({count})"),
        align: Align::Left,
        cells: rows.iter().map(|r| r.name.clone()).collect(),
    });
    if has_old {
        columns.push(SummaryColumn {
            header: "Old Version".to_string(),
            align: Align::Left,
            cells: rows.iter().map(|r| r.old_version.clone()).collect(),
        });
    }
    if has_new {
        columns.push(SummaryColumn {
            header: "New Version".to_string(),
            align: Align::Left,
            cells: rows.iter().map(|r| r.new_version.clone()).collect(),
        });
    }
    columns.push(SummaryColumn {
        header: "Net Change".to_string(),
        align: Align::Right,
        cells: rows.iter().map(|r| format_mib(r.net_bytes)).collect(),
    });
    if has_dl {
        columns.push(SummaryColumn {
            header: "Download Size".to_string(),
            align: Align::Right,
            cells: rows
                .iter()
                .map(|r| {
                    if r.download_bytes > 0 {
                        format_mib(r.download_bytes)
                    } else {
                        String::new()
                    }
                })
                .collect(),
        });
    }
    columns
}

fn column_widths(columns: &[SummaryColumn]) -> Vec<usize> {
    columns
        .iter()
        .map(|col| {
            let mut w = col.header.chars().count();
            for cell in &col.cells {
                w = w.max(cell.chars().count());
            }
            w
        })
        .collect()
}

fn append_table_line(
    out: &mut String,
    cells: &[String],
    aligns: &[Align],
    widths: &[usize],
    bold: bool,
    colored: bool,
) {
    for (i, (cell, width)) in cells.iter().zip(widths.iter()).enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let padded = if aligns[i] == Align::Right {
            format!("{:>t$}", cell, t = *width)
        } else {
            format!("{:<t$}", cell, t = *width)
        };
        if bold {
            out.push_str(&color::paint(colored, color::BOLD, &padded));
        } else {
            out.push_str(&padded);
        }
    }
    out.push('\n');
}

pub fn render_summary(summary: &TransactionSummary, colored: bool) -> String {
    let count = summary.packages.len();

    let rows = summary_rows(summary);
    let columns = build_columns(&rows, count);

    let widths: Vec<usize> = column_widths(&columns);

    let num_rows = rows.len();
    let mut out = String::new();

    out.push('\n');
    let headers: Vec<String> = columns.iter().map(|c| c.header.clone()).collect();
    let header_align: Vec<Align> = columns.iter().map(|_| Align::Left).collect();
    append_table_line(&mut out, &headers, &header_align, &widths, true, colored);
    out.push('\n');
    let data_align: Vec<Align> = columns.iter().map(|c| c.align).collect();
    for row_idx in 0..num_rows {
        let row_cells: Vec<String> = columns.iter().map(|c| c.cells[row_idx].clone()).collect();
        append_table_line(&mut out, &row_cells, &data_align, &widths, false, colored);
    }
    out.push('\n');

    append_footer(&mut out, summary, colored);

    out
}

fn append_footer(out: &mut String, summary: &TransactionSummary, colored: bool) {
    let dlsize = summary.total_download_size;
    let isize = summary.total_installed_size;
    let rsize = summary.total_removed_size;

    let mut rows: Vec<(String, String)> = Vec::new();
    if dlsize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Download Size:"),
            format_mib(dlsize),
        ));
    }
    if isize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Installed Size:"),
            format_mib(isize),
        ));
    }
    if rsize > 0 && isize == 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Total Removed Size:"),
            format_mib(rsize),
        ));
    }
    if isize > 0 && rsize > 0 {
        rows.push((
            color::paint(colored, color::BOLD, "Net Upgrade Size:"),
            format_mib(isize - rsize),
        ));
    }
    if rows.is_empty() {
        return;
    }

    let lw = rows
        .iter()
        .map(|(label, _)| color::visible_width(label))
        .max()
        .unwrap_or(0);
    let vw = rows
        .iter()
        .map(|(_, value)| color::visible_width(value))
        .max()
        .unwrap_or(0);
    for (label, value) in &rows {
        let lwt = lw
            + label
                .chars()
                .count()
                .saturating_sub(color::visible_width(label));
        out.push_str(&format!(
            "{:<lwt$}  {:>vw$}\n",
            label,
            value,
            lwt = lwt,
            vw = vw
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{render_summary, summary_rows};
    use crate::events::{SummaryPackage, TransactionSummary};

    fn install_package(name: &str, installed_size: i64, old_installed_size: i64) -> SummaryPackage {
        SummaryPackage {
            name: name.to_string(),
            repository: Some("extra".to_string()),
            new_version: "2.0-1".to_string(),
            old_version: Some("1.0-1".to_string()),
            download_size: 0,
            installed_size,
            old_installed_size,
            is_removal: false,
        }
    }

    #[test]
    fn summary_rows_net_bytes_for_removal() {
        let summary = TransactionSummary {
            packages: vec![SummaryPackage {
                name: "old".to_string(),
                repository: None,
                new_version: String::new(),
                old_version: Some("1.0-1".to_string()),
                download_size: 0,
                installed_size: 241591,
                old_installed_size: 0,
                is_removal: true,
            }],
            total_download_size: 0,
            total_installed_size: 0,
            total_removed_size: 241591,
        };
        let rows = summary_rows(&summary);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].net_bytes, -241591);
    }

    #[test]
    fn summary_rows_net_bytes_for_upgrade() {
        let summary = TransactionSummary {
            packages: vec![install_package("foo", 1048576, 786432)],
            total_download_size: 0,
            total_installed_size: 1048576,
            total_removed_size: 786432,
        };
        let rows = summary_rows(&summary);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].net_bytes, 1048576 - 786432);
    }

    #[test]
    fn summary_rows_ordering_removals_first_then_alphabetical() {
        let summary = TransactionSummary {
            packages: vec![
                install_package("zeta", 100, 0),
                SummaryPackage {
                    name: "gone".to_string(),
                    repository: None,
                    new_version: String::new(),
                    old_version: Some("1.0-1".to_string()),
                    download_size: 0,
                    installed_size: 50,
                    old_installed_size: 0,
                    is_removal: true,
                },
                install_package("alpha", 100, 0),
            ],
            total_download_size: 0,
            total_installed_size: 200,
            total_removed_size: 50,
        };
        let rows = summary_rows(&summary);
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["gone", "extra/alpha", "extra/zeta"]);
    }

    #[test]
    fn render_summary_matches_pacman_cava_case() {
        let installed = 199229;
        let removed = 241591;
        let summary = TransactionSummary {
            packages: vec![
                SummaryPackage {
                    name: "cava".to_string(),
                    repository: Some("extra".to_string()),
                    new_version: "0.10.7-1".to_string(),
                    old_version: None,
                    download_size: 0,
                    installed_size: installed,
                    old_installed_size: 0,
                    is_removal: false,
                },
                SummaryPackage {
                    name: "cava-git".to_string(),
                    repository: None,
                    new_version: String::new(),
                    old_version: Some("r1162.4b12c2b-1".to_string()),
                    download_size: 0,
                    installed_size: removed,
                    old_installed_size: 0,
                    is_removal: true,
                },
            ],
            total_download_size: 0,
            total_installed_size: installed,
            total_removed_size: removed,
        };

        let expected = [
            "",
            "Package (2)  Old Version      New Version  Net Change",
            "",
            "cava-git     r1162.4b12c2b-1                -0.23 MiB",
            "extra/cava                    0.10.7-1       0.19 MiB",
            "",
            "Total Installed Size:   0.19 MiB",
            "Net Upgrade Size:      -0.04 MiB",
        ]
        .join("\n")
            + "\n";

        let actual = render_summary(&summary, false);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }

    #[test]
    fn render_summary_matches_pacman_upgrade_case() {
        let new_isize = 1048576;
        let old_isize = 786432;
        let summary = TransactionSummary {
            packages: vec![SummaryPackage {
                name: "foo".to_string(),
                repository: Some("extra".to_string()),
                new_version: "2.0-1".to_string(),
                old_version: Some("1.0-1".to_string()),
                download_size: 0,
                installed_size: new_isize,
                old_installed_size: old_isize,
                is_removal: false,
            }],
            total_download_size: 0,
            total_installed_size: new_isize,
            total_removed_size: old_isize,
        };

        let expected = [
            "",
            "Package (1)  Old Version  New Version  Net Change",
            "",
            "extra/foo    1.0-1        2.0-1          0.25 MiB",
            "",
            "Total Installed Size:  1.00 MiB",
            "Net Upgrade Size:      0.25 MiB",
        ]
        .join("\n")
            + "\n";

        let actual = render_summary(&summary, false);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }

    #[test]
    fn render_summary_with_download_size_column() {
        let summary = TransactionSummary {
            packages: vec![SummaryPackage {
                name: "foo".to_string(),
                repository: Some("extra".to_string()),
                new_version: "2.0-1".to_string(),
                old_version: None,
                download_size: 524288,
                installed_size: 1048576,
                old_installed_size: 0,
                is_removal: false,
            }],
            total_download_size: 524288,
            total_installed_size: 1048576,
            total_removed_size: 0,
        };

        let expected = [
            "",
            "Package (1)  New Version  Net Change  Download Size",
            "",
            "extra/foo    2.0-1          1.00 MiB       0.50 MiB",
            "",
            "Total Download Size:   0.50 MiB",
            "Total Installed Size:  1.00 MiB",
        ]
        .join("\n")
            + "\n";

        let actual = render_summary(&summary, false);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }

    #[test]
    fn render_summary_colored_wide_name() {
        let summary = TransactionSummary {
            packages: vec![SummaryPackage {
                name: "something-longpkg".to_string(),
                repository: Some("extra".to_string()),
                new_version: "2.0-1".to_string(),
                old_version: Some("1.0-1".to_string()),
                download_size: 0,
                installed_size: 1048576,
                old_installed_size: 786432,
                is_removal: false,
            }],
            total_download_size: 0,
            total_installed_size: 1048576,
            total_removed_size: 786432,
        };
        let expected = "\n\x1b[0;1mPackage (1)            \x1b[0m  \x1b[0;1mOld Version\x1b[0m  \x1b[0;1mNew Version\x1b[0m  \x1b[0;1mNet Change\x1b[0m\n\nextra/something-longpkg  1.0-1        2.0-1          0.25 MiB\n\n\x1b[0;1mTotal Installed Size:\x1b[0m  1.00 MiB\n\x1b[0;1mNet Upgrade Size:\x1b[0m      0.25 MiB\n";
        let actual = render_summary(&summary, true);
        assert_eq!(actual, expected, "rendered summary table mismatch");
    }
}
