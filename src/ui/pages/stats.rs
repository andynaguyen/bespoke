//! The Stats page (`stats/StatsSettings.tsx`): totals for a chosen range,
//! the dictation streak, and a 20-week activity heatmap.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{
    anchored, deferred, div, point, prelude::*, px, rems, Context, Corner, FocusHandle, FontWeight,
    KeyDownEvent, Pixels, SharedString, Window,
};

use crate::app_handle::AppHandle;
use crate::i18n::{t, t_count};
use crate::managers::history::{DailyUsage, HistoryManager};
use crate::ui::components::{
    keyboard_modality, width_tracker, FocusVisible, RowDirection, Tabs, Tooltip, Typography,
};
use crate::ui::dates;
use crate::ui::format::number;
use crate::ui::theme::{palette, Alpha};

const HEATMAP_WEEKS: usize = 20;
const GAP: f32 = 3.75;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Range {
    Today,
    Week,
    Month,
    All,
}

impl Range {
    fn id(self) -> &'static str {
        match self {
            Range::Today => "today",
            Range::Week => "week",
            Range::Month => "month",
            Range::All => "all",
        }
    }

    fn days(self) -> Option<i64> {
        match self {
            Range::Today => Some(1),
            Range::Week => Some(7),
            Range::Month => Some(30),
            Range::All => None,
        }
    }
}

struct Summary {
    words: i64,
    transcriptions: i64,
    words_per_minute: Option<f64>,
}

/// Totals for days on or after `since`, or all days.
fn summarize(days: &[DailyUsage], since: Option<NaiveDate>) -> Summary {
    let mut summary = Summary {
        words: 0,
        transcriptions: 0,
        words_per_minute: None,
    };
    let (mut timed_words, mut duration_ms) = (0i64, 0i64);
    for day in days {
        let date = NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").ok();
        if let (Some(since), Some(date)) = (since, date) {
            if date < since {
                continue;
            }
        }
        summary.words += day.words;
        summary.transcriptions += day.transcriptions;
        timed_words += day.timed_words;
        duration_ms += day.duration_ms;
    }
    if duration_ms > 0 {
        summary.words_per_minute = Some(timed_words as f64 / (duration_ms as f64 / 60_000.));
    }
    summary
}

/// Current and longest runs of consecutive active days. The current streak
/// survives until a whole day passes without dictation.
fn streaks(active: &[NaiveDate], today: NaiveDate) -> (i64, i64) {
    let mut longest = 0;
    let mut run = 0;
    let mut previous: Option<NaiveDate> = None;
    for date in active {
        let continues = previous.is_some_and(|p| p + Duration::days(1) == *date);
        run = if continues { run + 1 } else { 1 };
        longest = longest.max(run);
        previous = Some(*date);
    }
    let set: HashSet<NaiveDate> = active.iter().copied().collect();
    let mut day = if set.contains(&today) {
        today
    } else {
        today - Duration::days(1)
    };
    let mut current = 0;
    while set.contains(&day) {
        current += 1;
        day -= Duration::days(1);
    }
    (current, longest)
}

/// Week columns, oldest first, Sunday to Saturday. `None` is a future day.
fn heatmap(
    words: &HashMap<NaiveDate, i64>,
    today: NaiveDate,
) -> Vec<Vec<Option<(NaiveDate, i64)>>> {
    let start = today
        - Duration::days(today.weekday().num_days_from_sunday() as i64)
        - Duration::days(((HEATMAP_WEEKS - 1) * 7) as i64);
    (0..HEATMAP_WEEKS)
        .map(|week| {
            (0..7)
                .map(|weekday| {
                    let date = start + Duration::days((week * 7 + weekday) as i64);
                    (date <= today).then(|| (date, words.get(&date).copied().unwrap_or(0)))
                })
                .collect()
        })
        .collect()
}

/// 0 for no activity, otherwise 1-4 relative to the busiest day.
fn intensity(words: i64, max: i64) -> usize {
    if words <= 0 || max <= 0 {
        return 0;
    }
    ((words as f64 / max as f64 * 4.).ceil() as usize).min(4)
}

pub struct StatsPage {
    days: Option<Result<Vec<DailyUsage>, String>>,
    range: Range,
    grid_width: Pixels,
    /// The heatmap cell that takes Tab focus; arrow keys move it. `None`
    /// means today.
    focus_date: Option<NaiveDate>,
    cell_focus: HashMap<NaiveDate, FocusHandle>,
}

impl StatsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            let result = crate::runtime::spawn_blocking(|| {
                AppHandle::global()
                    .state::<Arc<HistoryManager>>()
                    .get_daily_usage()
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = &result {
                    log::error!("Failed to load usage stats: {e}");
                }
                this.days = Some(result);
                cx.notify();
            });
        })
        .detach();
        Self {
            days: None,
            range: Range::Week,
            grid_width: px(0.),
            focus_date: None,
            cell_focus: HashMap::new(),
        }
    }
}

fn stat_card(value: String, label: SharedString, cx: &Context<StatsPage>) -> impl IntoElement {
    let p = palette(cx);
    div()
        .flex_1()
        .flex()
        .flex_col()
        .bg(p.card)
        .rounded_2xl()
        .px_5()
        .py_4()
        .child(
            div()
                .text_size(px(28.))
                .line_height(px(35.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value),
        )
        .child(
            div()
                .text_size(px(11.))
                .line_height(rems(1.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(p.text.a(0.55))
                .mt(rems(0.375))
                .child(label.to_uppercase()),
        )
}

impl Render for StatsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let root = div().w_full().flex().flex_col().gap_6();
        let days = match &self.days {
            None => {
                return root.child(
                    div()
                        .px_4()
                        .py_3()
                        .flex()
                        .row_dir()
                        .justify_center()
                        .text_color(p.text.a(0.6))
                        .child(t("settings.stats.loading")),
                )
            }
            Some(Err(_)) => {
                return root.child(
                    div()
                        .px_4()
                        .py_3()
                        .flex()
                        .row_dir()
                        .justify_center()
                        .text_color(p.text.a(0.6))
                        .child(t("settings.stats.loadError")),
                )
            }
            Some(Ok(days)) => days.clone(),
        };

        let today = Local::now().date_naive();
        let usage: HashMap<NaiveDate, DailyUsage> = days
            .iter()
            .filter_map(|d| {
                Some((
                    NaiveDate::parse_from_str(&d.date, "%Y-%m-%d").ok()?,
                    d.clone(),
                ))
            })
            .collect();
        let words: HashMap<NaiveDate, i64> = usage.iter().map(|(d, u)| (*d, u.words)).collect();
        let weeks = heatmap(&words, today);
        let max_words = weeks
            .iter()
            .flatten()
            .flatten()
            .map(|(_, w)| *w)
            .max()
            .unwrap_or(0);
        let mut active: Vec<NaiveDate> = usage.keys().copied().collect();
        active.sort();
        let (current, longest) = streaks(&active, today);

        let since = self.range.days().map(|n| today - Duration::days(n - 1));
        let summary = summarize(&days, since);

        let levels = [
            p.mid_gray.a(0.15),
            p.data.a(0.25),
            p.data.a(0.5),
            p.data.a(0.75),
            p.data,
        ];

        // A label column, then 20 equal columns of square cells sized from
        // the measured grid width.
        let label_width = px(26.);
        let cell = ((self.grid_width - label_width - px(GAP) * HEATMAP_WEEKS as f32)
            / HEATMAP_WEEKS as f32)
            .max(px(4.));
        let square = |el: gpui::Div| el.size(cell).flex_shrink_0();

        let month_row = div()
            .flex()
            .row_dir()
            .gap(px(GAP))
            .h(px(11.25))
            .child(div().w(label_width).flex_shrink_0())
            .children(weeks.iter().enumerate().map(|(index, week)| {
                let sunday = week[0].map(|(d, _)| d);
                let label = match (index, sunday) {
                    (0, _) | (_, None) => String::new(),
                    (_, Some(sunday)) => {
                        let previous = weeks[index - 1][0].map(|(d, _)| d);
                        if previous.is_some_and(|p| p.month() == sunday.month()) {
                            String::new()
                        } else {
                            dates::short_month(sunday)
                        }
                    }
                };
                div()
                    .w(cell)
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .child(label)
            }));

        // Roving focus: one cell is the tab stop, arrows move between cells.
        let dates: Vec<Vec<Option<NaiveDate>>> = weeks
            .iter()
            .map(|week| week.iter().map(|cell| cell.map(|(d, _)| d)).collect())
            .collect();
        for date in dates.iter().flatten().flatten() {
            self.cell_focus
                .entry(*date)
                .or_insert_with(|| cx.focus_handle());
        }
        let focused = dates
            .iter()
            .flatten()
            .flatten()
            .copied()
            .find(|d| self.cell_focus[d].is_focused(window));
        if focused.is_some() {
            self.focus_date = focused;
        }
        let tab_stop_date = self.focus_date.unwrap_or(today);
        let cell_ring = FocusVisible::ring_alpha(1., cx);
        let show_focused_card = keyboard_modality(cx);
        let rtl = crate::i18n::is_rtl();
        let dates = Arc::new(dates);

        let rows: Vec<_> = (0..7)
            .map(|weekday| {
                let label = if weekday % 2 == 1 {
                    weeks[0][weekday]
                        .map(|(d, _)| dates::short_weekday(d))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                div()
                    .flex()
                    .row_dir()
                    .items_center()
                    .gap(px(GAP))
                    .child(
                        div()
                            .w(label_width)
                            .flex_shrink_0()
                            .pe(rems(0.25))
                            .child(label),
                    )
                    .children(weeks.iter().enumerate().map(|(week_index, week)| {
                        let base = square(div());
                        match week[weekday] {
                            None => base.into_any_element(),
                            Some((date, words)) => {
                                let usage = usage.get(&date).cloned();
                                let handle = self.cell_focus[&date]
                                    .clone()
                                    .tab_stop(date == tab_stop_date);
                                let dates = dates.clone();
                                let cell = cell_ring
                                    .apply_with(
                                        base.id(SharedString::from(format!(
                                            "day-{week_index}-{weekday}"
                                        ))),
                                        handle,
                                    )
                                    .rounded(px(3.))
                                    .bg(levels[intensity(words, max_words)])
                                    .on_key_down(cx.listener(
                                        move |this, event: &KeyDownEvent, window, cx| {
                                            let (week_step, day_step) =
                                                match event.keystroke.key.as_str() {
                                                    "left" => (-1, 0),
                                                    "right" => (1, 0),
                                                    "up" => (0, -1),
                                                    "down" => (0, 1),
                                                    _ => return,
                                                };
                                            cx.stop_propagation();
                                            let week_step =
                                                if rtl { -week_step } else { week_step };
                                            let target = (week_index as isize + week_step)
                                                .try_into()
                                                .ok()
                                                .and_then(|w: usize| dates.get(w))
                                                .and_then(|week| {
                                                    (weekday as isize + day_step)
                                                        .try_into()
                                                        .ok()
                                                        .and_then(|d: usize| week.get(d).copied())
                                                })
                                                .flatten();
                                            if let Some(target) = target {
                                                if let Some(handle) = this.cell_focus.get(&target) {
                                                    window.focus(handle);
                                                }
                                                this.focus_date = Some(target);
                                                cx.notify();
                                            }
                                        },
                                    ));
                                if words == 0 {
                                    // Empty days get no card.
                                    return cell.into_any_element();
                                }
                                // The keyboard-focused day shows its card above
                                // the cell; the mouse gets the hover tooltip.
                                let focused_card = (show_focused_card && focused == Some(date))
                                    .then(|| {
                                        deferred(
                                            anchored()
                                                .anchor(Corner::BottomLeft)
                                                .offset(point(px(0.), px(-6.)))
                                                .snap_to_window_with_margin(px(8.))
                                                .child(day_popover(date, words, usage.clone(), cx)),
                                        )
                                        .with_priority(1)
                                    });
                                cell.tooltip(move |_, cx| {
                                    day_popover(date, words, usage.clone(), cx)
                                })
                                .children(focused_card)
                                .into_any_element()
                            }
                        }
                    }))
            })
            .collect();

        let tracker = width_tracker(cx.entity().downgrade(), |this: &mut Self| {
            &mut this.grid_width
        });

        let entity = cx.entity().downgrade();
        root.child(
            div()
                .flex()
                .flex_col()
                .gap_5()
                .child(Tabs::new(
                    "stats-ranges",
                    [Range::Today, Range::Week, Range::Month, Range::All]
                        .iter()
                        .map(|r| {
                            (
                                SharedString::from(r.id()),
                                t(&format!("settings.stats.ranges.{}", r.id())),
                            )
                        })
                        .collect(),
                    self.range.id(),
                    move |id, _, cx| {
                        let range = match id.as_ref() {
                            "today" => Range::Today,
                            "month" => Range::Month,
                            "all" => Range::All,
                            _ => Range::Week,
                        };
                        let _ = entity.update(cx, |this, cx| {
                            this.range = range;
                            cx.notify();
                        });
                    },
                ))
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .gap_3()
                        .child(stat_card(
                            summary
                                .words_per_minute
                                .map(|wpm| number(wpm.round() as i64))
                                .unwrap_or_else(|| "-".into()),
                            t("settings.stats.wordsPerMinute"),
                            cx,
                        ))
                        .child(stat_card(
                            number(summary.words),
                            t("settings.stats.totalWords"),
                            cx,
                        ))
                        .child(stat_card(
                            number(summary.transcriptions),
                            t("settings.stats.transcriptions"),
                            cx,
                        )),
                ),
        )
        .child(
            div()
                .bg(p.card)
                .rounded_2xl()
                .px_5()
                .py_5()
                .flex()
                .flex_col()
                .gap_5()
                .child(
                    div()
                        .flex()
                        .items_end()
                        .justify_between()
                        .row_dir()
                        .gap_4()
                        .child(
                            div()
                                .text_size(rems(1.5))
                                .line_height(rems(2.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t_count("settings.stats.streak", current, &[])),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text.a(0.7))
                                .child(
                                    t_count("settings.stats.longestStreak", longest, &[])
                                        .to_uppercase(),
                                ),
                        ),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .text_size(px(10.))
                        .line_height(px(10.))
                        .text_color(p.text.a(0.5))
                        .child(month_row)
                        .children(rows)
                        .child(tracker),
                )
                .child(
                    div()
                        .flex()
                        .row_dir()
                        .items_center()
                        .justify_flex_end()
                        .gap_1()
                        .xs_text()
                        .text_color(p.text.a(0.5))
                        .child(div().me(rems(0.25)).child(t("settings.stats.less")))
                        .children(
                            levels
                                .iter()
                                .map(|color| div().size(rems(0.75)).rounded(px(3.)).bg(*color)),
                        )
                        .child(div().ms(rems(0.25)).child(t("settings.stats.more"))),
                ),
        )
    }
}

/// The hover card for a heatmap day.
fn day_popover(
    date: NaiveDate,
    words: i64,
    usage: Option<DailyUsage>,
    cx: &mut gpui::App,
) -> gpui::AnyView {
    Tooltip::element(
        move |_, cx| {
            let p = palette(cx);
            let row = |label: SharedString, value: String| {
                div()
                    .flex()
                    .justify_between()
                    .row_dir()
                    .gap_3()
                    .child(
                        div()
                            .text_color(p.text.a(0.6))
                            .whitespace_nowrap()
                            .child(label),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(value),
                    )
            };
            let mut rows = vec![row(t("settings.stats.dayPopover.words"), number(words))];
            if let Some(usage) = usage.as_ref().filter(|u| u.top_app.is_some()) {
                rows.push(row(
                    t("settings.stats.dayPopover.appsUsed"),
                    number(usage.apps_used),
                ));
                rows.push(row(
                    t("settings.stats.dayPopover.topApp"),
                    usage.top_app.clone().unwrap_or_default(),
                ));
            }
            div()
                .xs_text()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .sm_text()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(dates::long_date(date)),
                )
                .child(div().flex().flex_col().gap_1().children(rows))
                .into_any_element()
        },
        cx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn current_streak_survives_until_a_day_is_missed() {
        let active = [
            d("2025-01-01"),
            d("2025-01-02"),
            d("2025-01-03"),
            d("2025-01-05"),
        ];
        assert_eq!(streaks(&active, d("2025-01-05")), (1, 3));
        assert_eq!(streaks(&active, d("2025-01-06")), (1, 3));
        assert_eq!(streaks(&active, d("2025-01-07")), (0, 3));
    }

    #[test]
    fn intensity_buckets_relative_to_busiest_day() {
        assert_eq!(intensity(0, 100), 0);
        assert_eq!(intensity(1, 100), 1);
        assert_eq!(intensity(100, 100), 4);
    }
}
