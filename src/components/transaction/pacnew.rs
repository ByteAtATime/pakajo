use cosmic::iced::Color;
use cosmic::iced::Length;
use cosmic::iced::alignment::Vertical;
use cosmic::iced::widget::{rich_text, span};
use cosmic::widget::{button, checkbox, container, dialog, divider, scrollable, space, text};

use super::diff::diff_rows_column;
use super::shared::{mono_text, muted, muted_color, pill, version_change, warning_color};
use crate::Element;
use crate::components::icons;
use crate::components::theme::muted_mono;
use pakajo::diff::{DiffLine, RenderedLine, rendered_lines_seeded, syntax_for_file};
use pakajo::events::{MergeHunk, MergeLine, MergeOrigin};
use pakajo::tx::pacnew::MergeKind;

const LEARN_MORE_URL: &str = "https://wiki.archlinux.org/title/Pacman/Pacnew_and_Pacsave";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MergeMessage {
    KeepCurrent,
    Restore,
    Delete,
    Defer,
    ToggleHunk(usize),
    ApplyMerge,
}

#[derive(Clone, Debug)]
pub struct MergeOffer {
    pub kind: MergeKind,
    pub file: String,
    pub from_noupgrade: bool,
    pub hunks: Vec<MergeHunk>,
    pub origin: Option<MergeOrigin>,
    pub index: usize,
    pub total: usize,
}

#[derive(Clone, Debug)]
pub struct OriginRow {
    pub package: String,
    pub transition: String,
}

#[derive(Clone, Debug)]
pub struct HunkView {
    pub rows: Vec<RenderedLine>,
    pub numbers: Vec<Option<usize>>,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct MergeView {
    pub kind: MergeKind,
    pub file: String,
    pub from_noupgrade: bool,
    pub hunks: Vec<HunkView>,
    pub origin: Option<OriginRow>,
    pub index: usize,
    pub total: usize,
}

pub fn origin_row(origin: &Option<MergeOrigin>) -> Option<OriginRow> {
    let origin = origin.as_ref()?;
    if origin.old_version.is_none() && origin.new_version.is_none() {
        return None;
    }
    Some(OriginRow {
        package: origin.package.clone(),
        transition: version_change(origin.old_version.as_deref(), origin.new_version.as_deref()),
    })
}

pub fn format_change_label(number: usize) -> String {
    format!("Change {number}")
}

pub fn format_apply_label(checked: usize) -> String {
    if checked == 1 {
        "Apply 1 change".to_string()
    } else {
        format!("Apply {checked} changes")
    }
}

pub fn position_label(index: usize, total: usize) -> Option<String> {
    if total <= 1 {
        return None;
    }
    Some(format!("File {index} of {total}"))
}

pub fn action_label<'a>(
    pressed: Option<&MergeMessage>,
    button: &MergeMessage,
    normal: &'a str,
) -> &'a str {
    if pressed == Some(button) {
        "Loading..."
    } else {
        normal
    }
}

fn intro_text(kind: MergeKind) -> &'static str {
    match kind {
        MergeKind::Pacnew => {
            "You just updated a package that ships a new default config for this file. Review the changes below and pick the ones you want to apply."
        }
        MergeKind::Pacsave => {
            "You just removed a package that had this config backed up. Review the file below and choose to restore it or delete it."
        }
    }
}

fn hunk_diff_lines(hunk: &MergeHunk) -> Vec<DiffLine> {
    hunk.before
        .iter()
        .map(|line| DiffLine::Context(line.clone()))
        .chain(hunk.lines.iter().map(|line| match line {
            MergeLine::Added(body) => DiffLine::Added(body.clone()),
            MergeLine::Removed(body) => DiffLine::Removed(body.clone()),
        }))
        .chain(
            hunk.after
                .iter()
                .map(|line| DiffLine::Context(line.clone())),
        )
        .collect()
}

fn hunk_numbers(hunk: &MergeHunk) -> Vec<Option<usize>> {
    let mut numbers = Vec::with_capacity(hunk.before.len() + hunk.lines.len() + hunk.after.len());
    let before_base = hunk.new_start.saturating_sub(hunk.before.len());
    numbers.extend((0..hunk.before.len()).map(|offset| Some(before_base + offset)));
    let mut old = hunk.old_start;
    let mut new = hunk.new_start;
    for line in &hunk.lines {
        match line {
            MergeLine::Removed(_) => {
                numbers.push(Some(old));
                old += 1;
            }
            MergeLine::Added(_) => {
                numbers.push(Some(new));
                new += 1;
            }
        }
    }
    let after_base = hunk.new_start + hunk.new_len;
    numbers.extend((0..hunk.after.len()).map(|offset| Some(after_base + offset)));
    numbers
}

impl MergeView {
    pub fn parse(offer: &MergeOffer, dark: bool) -> Self {
        let syntax = syntax_for_file(&offer.file);
        let hunks = offer
            .hunks
            .iter()
            .map(|hunk| {
                let diff = hunk_diff_lines(hunk);
                HunkView {
                    rows: rendered_lines_seeded(&diff, false, dark, syntax),
                    numbers: hunk_numbers(hunk),
                    selected: true,
                }
            })
            .collect();
        Self {
            kind: offer.kind,
            file: offer.file.clone(),
            from_noupgrade: offer.from_noupgrade,
            hunks,
            origin: origin_row(&offer.origin),
            index: offer.index,
            total: offer.total,
        }
    }

    pub fn checked_hunks(&self) -> Vec<usize> {
        self.hunks
            .iter()
            .enumerate()
            .filter(|(_, hunk)| hunk.selected)
            .map(|(index, _)| index)
            .collect()
    }

    pub fn toggle_hunk(&mut self, index: usize) {
        if let Some(hunk) = self.hunks.get_mut(index) {
            hunk.selected = !hunk.selected;
        }
    }
}

fn number_cell(number: Option<usize>) -> Element<'static> {
    let label = number.map_or(String::new(), |number| number.to_string());
    container(muted_mono(label))
        .width(Length::Fixed(40.0))
        .into()
}

fn numbered_row(row: &RenderedLine, number: Option<usize>) -> Element<'_> {
    if matches!(row, RenderedLine::Code { .. }) {
        cosmic::widget::Row::new()
            .spacing(8)
            .align_y(Vertical::Center)
            .push(number_cell(number))
            .push(diff_rows_column(std::slice::from_ref(row), false))
            .into()
    } else {
        diff_rows_column(std::slice::from_ref(row), false)
    }
}

fn numbered_rows_column<'a>(rows: &'a [RenderedLine], numbers: &'a [Option<usize>]) -> Element<'a> {
    let mut column = cosmic::widget::Column::new().spacing(0);
    for (index, row) in rows.iter().enumerate() {
        let number = numbers.get(index).copied().flatten();
        column = column.push(numbered_row(row, number));
    }
    column.into()
}

fn hunk_sections_column<'a>(
    view: &'a MergeView,
    locked: bool,
    to_message: std::rc::Rc<impl Fn(MergeMessage) -> crate::Message + 'a>,
) -> Element<'a> {
    let mut column = cosmic::widget::Column::new().spacing(12);
    for (index, hunk) in view.hunks.iter().enumerate() {
        let mut toggle = checkbox(hunk.selected).label(format_change_label(index + 1));
        if !locked {
            let forward = std::rc::Rc::clone(&to_message);
            toggle = toggle.on_toggle(move |_| forward(MergeMessage::ToggleHunk(index)));
        }
        let section = cosmic::widget::Column::new()
            .spacing(4)
            .push(toggle)
            .push(numbered_rows_column(&hunk.rows, &hunk.numbers));
        column = column.push(section);
    }
    scrollable(column).height(Length::Fill).into()
}

fn pacsave_body_column(view: &MergeView) -> Element<'_> {
    let mut column = cosmic::widget::Column::new().spacing(12);
    for hunk in &view.hunks {
        column = column.push(numbered_rows_column(&hunk.rows, &hunk.numbers));
    }
    scrollable(column).height(Length::Fill).into()
}

fn meta_row(view: &MergeView) -> Option<Element<'_>> {
    if view.origin.is_none() && !view.from_noupgrade {
        return None;
    }
    let mut meta = cosmic::widget::Row::new()
        .align_y(Vertical::Center)
        .spacing(8);
    if let Some(origin) = view.origin.as_ref() {
        meta = meta
            .push(cosmic::widget::icon(icons::package()).size(16))
            .push(mono_text(&origin.package))
            .push(muted(muted_mono(origin.transition.clone())));
    }
    if view.from_noupgrade {
        meta = meta.push(pill(
            "new version was not installed (NoUpgrade)",
            warning_color,
        ));
    }
    Some(meta.into())
}

fn merge_header(view: &MergeView) -> Element<'_> {
    let mut line = cosmic::widget::Row::new()
        .spacing(12)
        .align_y(Vertical::Center);
    if let Some(label) = position_label(view.index, view.total) {
        line = line.push(pill(label, muted_color));
    }
    line = line.push(
        text::heading(&view.file)
            .font(cosmic::font::mono())
            .width(Length::Fill),
    );
    if let Some(meta) = meta_row(view) {
        line = line.push(meta);
    }
    line.into()
}

fn merge_intro(kind: MergeKind) -> Element<'static> {
    let theme = cosmic::theme::active();
    let accent = Color::from(theme.cosmic().accent_text_color());
    rich_text::<String, crate::Message, cosmic::Theme, cosmic::Renderer>([
        span(intro_text(kind)),
        span(" "),
        span("Learn more →")
            .color(accent)
            .underline(true)
            .link(LEARN_MORE_URL.to_string()),
    ])
    .line_height(1.6)
    .width(Length::Fill)
    .on_link_click(crate::Message::OpenUrl)
    .into()
}

pub(super) fn merge_dialog<'a>(
    view: &'a MergeView,
    pressed: Option<MergeMessage>,
    to_message: impl Fn(MergeMessage) -> crate::Message + 'a,
) -> Element<'a> {
    let to_message = std::rc::Rc::new(to_message);
    let intro = merge_intro(view.kind);
    let header = merge_header(view);
    let locked = pressed.is_some();
    let scroll: Element<'_> = if view.hunks.is_empty() {
        text("No text diff available").into()
    } else if matches!(view.kind, MergeKind::Pacnew) {
        hunk_sections_column(view, locked, std::rc::Rc::clone(&to_message))
    } else {
        pacsave_body_column(view)
    };
    let column = cosmic::widget::Column::new()
        .push(intro)
        .push(space::vertical().height(Length::Fixed(24.0)))
        .push(header)
        .push(space::vertical().height(Length::Fixed(8.0)))
        .push(divider::horizontal::default())
        .push(space::vertical().height(Length::Fixed(12.0)))
        .push(container(scroll).width(Length::Fill).height(Length::Fill));
    let body = container(column).width(Length::Fill).height(Length::Fill);
    let shell = dialog()
        .title("Review Config Changes")
        .control(body)
        .width(Length::Fill)
        .max_width(1100.0)
        .height(Length::Fill)
        .max_height(800.0);
    match view.kind {
        MergeKind::Pacnew => {
            let checked = view.checked_hunks().len();
            let apply_normal = format_apply_label(checked);
            let apply_text =
                action_label(pressed.as_ref(), &MergeMessage::ApplyMerge, &apply_normal)
                    .to_string();
            let mut apply = button::suggested(apply_text);
            if !locked && checked > 0 {
                apply = apply.on_press(to_message(MergeMessage::ApplyMerge));
            }
            let mut keep = button::standard(action_label(
                pressed.as_ref(),
                &MergeMessage::KeepCurrent,
                "Keep Existing",
            ));
            if !locked {
                keep = keep.on_press(to_message(MergeMessage::KeepCurrent));
            }
            let mut defer = button::standard(action_label(
                pressed.as_ref(),
                &MergeMessage::Defer,
                "Decide Later",
            ));
            if !locked {
                defer = defer.on_press(to_message(MergeMessage::Defer));
            }
            shell
                .primary_action(apply)
                .secondary_action(keep)
                .tertiary_action(defer)
                .into()
        }
        MergeKind::Pacsave => {
            let mut restore = button::suggested(action_label(
                pressed.as_ref(),
                &MergeMessage::Restore,
                "Restore",
            ));
            if !locked {
                restore = restore.on_press(to_message(MergeMessage::Restore));
            }
            let mut delete = button::standard(action_label(
                pressed.as_ref(),
                &MergeMessage::Delete,
                "Delete",
            ));
            if !locked {
                delete = delete.on_press(to_message(MergeMessage::Delete));
            }
            let mut defer = button::standard(action_label(
                pressed.as_ref(),
                &MergeMessage::Defer,
                "Keep for later",
            ));
            if !locked {
                defer = defer.on_press(to_message(MergeMessage::Defer));
            }
            shell
                .primary_action(restore)
                .secondary_action(delete)
                .tertiary_action(defer)
                .into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(kind: MergeKind, file: &str, hunks: Vec<MergeHunk>) -> MergeOffer {
        MergeOffer {
            kind,
            file: file.to_string(),
            from_noupgrade: false,
            hunks,
            origin: None,
            index: 1,
            total: 1,
        }
    }

    fn hunk(
        old_start: usize,
        new_start: usize,
        before: &[&str],
        lines: Vec<MergeLine>,
        after: &[&str],
    ) -> MergeHunk {
        MergeHunk {
            old_start,
            old_len: lines
                .iter()
                .filter(|line| matches!(line, MergeLine::Removed(_)))
                .count(),
            new_start,
            new_len: lines
                .iter()
                .filter(|line| matches!(line, MergeLine::Added(_)))
                .count(),
            before: before.iter().map(|line| line.to_string()).collect(),
            after: after.iter().map(|line| line.to_string()).collect(),
            lines,
        }
    }

    fn removed(body: &str) -> MergeLine {
        MergeLine::Removed(body.to_string())
    }

    fn added(body: &str) -> MergeLine {
        MergeLine::Added(body.to_string())
    }

    fn displays(view: &MergeView, hunk: usize) -> Vec<Option<usize>> {
        view.hunks[hunk].numbers.clone()
    }

    #[test]
    fn mixed_hunk_shows_old_numbers_on_removed_and_new_numbers_on_added() {
        let view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![hunk(
                    10,
                    20,
                    &[],
                    vec![
                        removed("old-a"),
                        removed("old-b"),
                        added("new-a"),
                        added("new-b"),
                    ],
                    &[],
                )],
            ),
            true,
        );
        assert_eq!(
            displays(&view, 0),
            vec![Some(10), Some(11), Some(20), Some(21)]
        );
    }

    #[test]
    fn before_flank_counts_down_to_new_start_minus_one() {
        let view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![hunk(
                    7,
                    9,
                    &["one", "two", "three"],
                    vec![added("new")],
                    &[],
                )],
            ),
            true,
        );
        assert_eq!(displays(&view, 0), vec![Some(6), Some(7), Some(8), Some(9)]);
    }

    #[test]
    fn after_flank_counts_up_from_new_start_plus_new_len() {
        let view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![hunk(4, 4, &[], vec![removed("gone")], &["one", "two"])],
            ),
            true,
        );
        assert_eq!(displays(&view, 0), vec![Some(4), Some(4), Some(5)]);
    }

    #[test]
    fn all_hunks_start_checked() {
        let view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![
                    hunk(1, 1, &[], vec![added("a")], &[]),
                    hunk(5, 5, &[], vec![added("b")], &[]),
                ],
            ),
            true,
        );
        assert_eq!(view.checked_hunks(), vec![0, 1]);
    }

    #[test]
    fn toggle_hunk_flips_selection() {
        let mut view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![hunk(1, 1, &[], vec![added("a")], &[])],
            ),
            true,
        );
        view.toggle_hunk(0);
        assert_eq!(view.checked_hunks(), Vec::<usize>::new());
        view.toggle_hunk(0);
        assert_eq!(view.checked_hunks(), vec![0]);
    }

    #[test]
    fn checked_hunks_returns_indices_in_order() {
        let mut view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![
                    hunk(1, 1, &[], vec![added("a")], &[]),
                    hunk(5, 5, &[], vec![added("b")], &[]),
                    hunk(9, 9, &[], vec![added("c")], &[]),
                ],
            ),
            true,
        );
        view.toggle_hunk(1);
        assert_eq!(view.checked_hunks(), vec![0, 2]);
    }

    #[test]
    fn parse_row_count_sums_flanks_and_lines_across_hunks() {
        let view = MergeView::parse(
            &offer(
                MergeKind::Pacnew,
                "/etc/app.conf",
                vec![
                    hunk(1, 1, &["top"], vec![removed("a"), added("b")], &["mid"]),
                    hunk(5, 5, &[], vec![added("c")], &["tail", "end"]),
                ],
            ),
            true,
        );
        assert_eq!(view.hunks.len(), 2);
        assert_eq!(view.hunks[0].rows.len(), 4);
        assert_eq!(view.hunks[0].numbers.len(), 4);
        assert_eq!(view.hunks[1].rows.len(), 3);
        assert_eq!(view.hunks[1].numbers.len(), 3);
    }

    #[test]
    fn parse_carries_offer_identity_onto_view() {
        let mut base = offer(
            MergeKind::Pacsave,
            "/etc/hosts",
            vec![hunk(1, 1, &[], vec![added("a")], &[])],
        );
        base.from_noupgrade = true;
        base.origin = Some(MergeOrigin {
            package: "filesystem".to_string(),
            old_version: Some("1.0-1".to_string()),
            new_version: Some("2.0-1".to_string()),
        });
        base.index = 2;
        base.total = 3;
        let view = MergeView::parse(&base, false);
        assert_eq!(view.kind, MergeKind::Pacsave);
        assert_eq!(view.file, "/etc/hosts");
        assert!(view.from_noupgrade);
        assert_eq!(view.index, 2);
        assert_eq!(view.total, 3);
        let origin = view.origin.expect("origin present");
        assert_eq!(origin.package, "filesystem");
        assert_eq!(origin.transition, "1.0-1 → 2.0-1");
    }

    #[test]
    fn apply_label_uses_singular_for_one_change() {
        assert_eq!(format_apply_label(1), "Apply 1 change");
    }

    #[test]
    fn apply_label_uses_plural_otherwise() {
        assert_eq!(format_apply_label(0), "Apply 0 changes");
        assert_eq!(format_apply_label(2), "Apply 2 changes");
    }

    #[test]
    fn position_label_shows_counter_for_multi_file_offers() {
        assert_eq!(position_label(2, 3), Some("File 2 of 3".to_string()));
    }

    #[test]
    fn position_label_hides_counter_for_single_file_offers() {
        assert_eq!(position_label(1, 1), None);
    }

    #[test]
    fn origin_row_builds_package_transition() {
        let row = origin_row(&Some(MergeOrigin {
            package: "pacman".to_string(),
            old_version: Some("6.0-1".to_string()),
            new_version: Some("7.0-1".to_string()),
        }))
        .expect("origin present");
        assert_eq!(row.package, "pacman");
        assert_eq!(row.transition, "6.0-1 → 7.0-1");
    }

    #[test]
    fn origin_row_is_empty_without_versions() {
        assert!(origin_row(&None).is_none());
        assert!(
            origin_row(&Some(MergeOrigin {
                package: "pacman".to_string(),
                old_version: None,
                new_version: None,
            }))
            .is_none()
        );
    }

    #[test]
    fn change_label_numbers_each_hunk() {
        assert_eq!(format_change_label(1), "Change 1");
        assert_eq!(format_change_label(2), "Change 2");
    }

    #[test]
    fn intro_copy_points_at_wiki_page_for_both_kinds() {
        assert_eq!(
            intro_text(MergeKind::Pacnew),
            "You just updated a package that ships a new default config for this file. Review the changes below and pick the ones you want to apply."
        );
        assert_eq!(
            intro_text(MergeKind::Pacsave),
            "You just removed a package that had this config backed up. Review the file below and choose to restore it or delete it."
        );
        assert_eq!(
            LEARN_MORE_URL,
            "https://wiki.archlinux.org/title/Pacman/Pacnew_and_Pacsave"
        );
    }

    #[test]
    fn merge_dialog_builds_every_branch() {
        let mut pacnew = offer(
            MergeKind::Pacnew,
            "/etc/app.conf",
            vec![
                hunk(1, 1, &["top"], vec![removed("a"), added("b")], &[]),
                hunk(5, 5, &[], vec![added("c")], &["tail"]),
            ],
        );
        pacnew.from_noupgrade = true;
        pacnew.origin = Some(MergeOrigin {
            package: "app".to_string(),
            old_version: Some("1.0-1".to_string()),
            new_version: Some("2.0-1".to_string()),
        });
        pacnew.total = 2;
        let pacnew = MergeView::parse(&pacnew, true);
        let pacsave = MergeView::parse(
            &offer(
                MergeKind::Pacsave,
                "/etc/hosts",
                vec![hunk(1, 1, &[], vec![removed("gone")], &[])],
            ),
            true,
        );
        let empty = MergeView::parse(&offer(MergeKind::Pacnew, "/etc/app.conf", Vec::new()), true);
        let forward = |_: MergeMessage| crate::Message::DbLockReleased;
        drop(merge_dialog(&pacnew, None, forward));
        drop(merge_dialog(
            &pacnew,
            Some(MergeMessage::ApplyMerge),
            forward,
        ));
        drop(merge_dialog(
            &pacnew,
            Some(MergeMessage::KeepCurrent),
            forward,
        ));
        drop(merge_dialog(&pacnew, Some(MergeMessage::Defer), forward));
        drop(merge_dialog(
            &pacnew,
            Some(MergeMessage::ToggleHunk(0)),
            forward,
        ));
        drop(merge_dialog(&pacsave, None, forward));
        drop(merge_dialog(&pacsave, Some(MergeMessage::Restore), forward));
        drop(merge_dialog(&pacsave, Some(MergeMessage::Delete), forward));
        drop(merge_dialog(&empty, None, forward));
    }

    #[test]
    fn action_label_shows_loading_for_pressed_button_only() {
        assert_eq!(
            action_label(
                Some(&MergeMessage::ApplyMerge),
                &MergeMessage::ApplyMerge,
                "Apply 2 changes"
            ),
            "Loading..."
        );
        assert_eq!(
            action_label(
                Some(&MergeMessage::ApplyMerge),
                &MergeMessage::KeepCurrent,
                "Keep Existing"
            ),
            "Keep Existing"
        );
        assert_eq!(
            action_label(None, &MergeMessage::Defer, "Decide Later"),
            "Decide Later"
        );
    }
}
