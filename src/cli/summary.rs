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

struct SummaryColumn {
    header: String,
    right_align_data: bool,
    cells: Vec<String>,
}

fn append_table_line(out: &mut String, cells: &[String], right_align: &[bool], widths: &[usize]) {
    for (i, (cell, width)) in cells.iter().zip(widths.iter()).enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let target = *width
            + cell
                .chars()
                .count()
                .saturating_sub(color::visible_width(cell));
        if right_align[i] {
            out.push_str(&format!("{:>t$}", cell, t = target));
        } else {
            out.push_str(&format!("{:<t$}", cell, t = target));
        }
    }
    out.push('\n');
}

pub fn render_summary(summary: &TransactionSummary, colored: bool) -> String {
    let count = summary.packages.len();

    let mut ordered: Vec<&SummaryPackage> = summary.packages.iter().collect();
    ordered.sort_by_key(|p| (!p.is_removal, p.name.clone()));

    let rows: Vec<(String, String, String, String, String)> = ordered
        .iter()
        .map(|p| {
            let net = if p.is_removal {
                -p.installed_size
            } else {
                p.installed_size - p.old_installed_size
            };
            let dl = if p.download_size > 0 {
                format_mib(p.download_size)
            } else {
                String::new()
            };
            (
                formatted_name(p),
                p.old_version.clone().unwrap_or_default(),
                p.new_version.clone(),
                format_mib(net),
                dl,
            )
        })
        .collect();

    let has_old = rows.iter().any(|(_, old, _, _, _)| !old.is_empty());
    let has_new = rows.iter().any(|(_, _, new, _, _)| !new.is_empty());
    let has_dl = rows.iter().any(|(_, _, _, _, dl)| !dl.is_empty());

    let mut columns: Vec<SummaryColumn> = Vec::new();
    columns.push(SummaryColumn {
        header: format!("Package ({count})"),
        right_align_data: false,
        cells: rows.iter().map(|(name, _, _, _, _)| name.clone()).collect(),
    });
    if has_old {
        columns.push(SummaryColumn {
            header: "Old Version".to_string(),
            right_align_data: false,
            cells: rows.iter().map(|(_, old, _, _, _)| old.clone()).collect(),
        });
    }
    if has_new {
        columns.push(SummaryColumn {
            header: "New Version".to_string(),
            right_align_data: false,
            cells: rows.iter().map(|(_, _, new, _, _)| new.clone()).collect(),
        });
    }
    columns.push(SummaryColumn {
        header: "Net Change".to_string(),
        right_align_data: true,
        cells: rows.iter().map(|(_, _, _, net, _)| net.clone()).collect(),
    });
    if has_dl {
        columns.push(SummaryColumn {
            header: "Download Size".to_string(),
            right_align_data: true,
            cells: rows.iter().map(|(_, _, _, _, dl)| dl.clone()).collect(),
        });
    }

    let widths: Vec<usize> = columns
        .iter()
        .map(|col| {
            let mut w = col.header.len();
            for cell in &col.cells {
                w = w.max(cell.len());
            }
            w
        })
        .collect();

    let num_rows = rows.len();
    let mut out = String::new();

    out.push('\n');
    let header_cells: Vec<String> = columns
        .iter()
        .map(|c| color::paint(colored, color::BOLD, &c.header))
        .collect();
    let header_align: Vec<bool> = columns.iter().map(|_| false).collect();
    append_table_line(&mut out, &header_cells, &header_align, &widths);
    out.push('\n');
    let data_align: Vec<bool> = columns.iter().map(|c| c.right_align_data).collect();
    for row_idx in 0..num_rows {
        let row_cells: Vec<String> = columns.iter().map(|c| c.cells[row_idx].clone()).collect();
        append_table_line(&mut out, &row_cells, &data_align, &widths);
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

fn formatted_name(pkg: &SummaryPackage) -> String {
    match &pkg.repository {
        Some(repo) => format!("{repo}/{}", pkg.name),
        None => pkg.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::render_summary;
    use crate::events::{SummaryPackage, TransactionSummary};

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
}
