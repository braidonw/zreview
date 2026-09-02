//! PROTOTYPE. Throwaway. Three layouts for Home, on fixture data.
//!
//! Run with `cargo run -p zreview --bin home_prototype [a|b|c]`. Left and right
//! switch variants, `j`/`k` move the cursor, `e` empties the repositories, `s`
//! toggles the hidden Session, `g` empties the To address group.

#![allow(clippy::unreadable_literal, clippy::too_many_lines)]

use std::env;

use gpui::{
    App, AppContext, Application, Bounds, Context, FocusHandle, Focusable, FontWeight, KeyBinding,
    MouseButton, Render, SharedString, Window, WindowBounds, WindowOptions, actions, div,
    prelude::*, px, rgb, size,
};
use ui::theme;

actions!(
    home_prototype,
    [
        NextVariant,
        PreviousVariant,
        CursorDown,
        CursorUp,
        ToggleRepositories,
        ToggleHiddenSession,
        ToggleEmptyGroup,
    ]
);

const CONTEXT: &str = "HomePrototype";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Ledger,
    Sidebar,
    Columns,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::Ledger, Variant::Sidebar, Variant::Columns];

    fn key(self) -> &'static str {
        match self {
            Variant::Ledger => "A",
            Variant::Sidebar => "B",
            Variant::Columns => "C",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Variant::Ledger => "Ledger, repositories in the footer",
            Variant::Sidebar => "Sidebar, repositories on the left",
            Variant::Columns => "Columns, repositories as chips",
        }
    }

    fn next(self) -> Variant {
        let index = Variant::ALL.iter().position(|v| *v == self).unwrap();
        Variant::ALL[(index + 1) % Variant::ALL.len()]
    }

    fn previous(self) -> Variant {
        let index = Variant::ALL.iter().position(|v| *v == self).unwrap();
        Variant::ALL[(index + Variant::ALL.len() - 1) % Variant::ALL.len()]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    ToReview,
    ToAddress,
    Waiting,
}

impl Group {
    const ALL: [Group; 3] = [Group::ToReview, Group::ToAddress, Group::Waiting];

    fn title(self) -> &'static str {
        match self {
            Group::ToReview => "To review",
            Group::ToAddress => "To address",
            Group::Waiting => "Waiting on others",
        }
    }

    fn empty_copy(self) -> &'static str {
        match self {
            Group::ToReview => "Nothing waiting for your review.",
            Group::ToAddress => "Nothing to address.",
            Group::Waiting => "Nothing waiting on others.",
        }
    }
}

#[derive(Clone, Copy)]
enum Checks {
    Passing,
    Failing,
    Pending,
    None,
}

#[derive(Clone, Copy)]
enum ReviewStatus {
    None,
    ReviewedThisHead,
    ChangesRequested,
    Approved,
}

struct Row {
    group: Group,
    repository: &'static str,
    number: u32,
    title: &'static str,
    author: &'static str,
    updated: &'static str,
    checks: Checks,
    review: ReviewStatus,
    drafts: usize,
    /// The Session kept alive behind Home, with a review run in flight.
    hidden_session: bool,
}

enum RepositoryState {
    Loaded(usize),
    Loading,
    Failed(&'static str),
}

struct Repository {
    slug: &'static str,
    path: &'static str,
    state: RepositoryState,
}

const REPOSITORIES: [Repository; 4] = [
    Repository {
        slug: "braidonw/zreview",
        path: "~/Developer/zreview",
        state: RepositoryState::Loaded(3),
    },
    Repository {
        slug: "acme/widgets",
        path: "~/Developer/widgets",
        state: RepositoryState::Loaded(5),
    },
    Repository {
        slug: "acme/billing-service",
        path: "~/Developer/billing",
        state: RepositoryState::Failed("gh: HTTP 403: resource protected by organization SAML enforcement"),
    },
    Repository {
        slug: "acme/design-tokens",
        path: "~/Developer/design-tokens",
        state: RepositoryState::Loading,
    },
];

const ROWS: [Row; 8] = [
    Row {
        group: Group::ToReview,
        repository: "acme/widgets",
        number: 412,
        title: "Retry webhook deliveries with jittered backoff",
        author: "mlee",
        updated: "2h",
        checks: Checks::Passing,
        review: ReviewStatus::None,
        drafts: 2,
        hidden_session: true,
    },
    Row {
        group: Group::ToReview,
        repository: "acme/widgets",
        number: 398,
        title: "Split the invoice PDF renderer out of the API crate",
        author: "priya",
        updated: "1d",
        checks: Checks::Failing,
        review: ReviewStatus::ReviewedThisHead,
        drafts: 0,
        hidden_session: false,
    },
    Row {
        group: Group::ToReview,
        repository: "acme/widgets",
        number: 405,
        title: "Drop the legacy CSV importer",
        author: "tomas",
        updated: "3d",
        checks: Checks::Pending,
        review: ReviewStatus::None,
        drafts: 0,
        hidden_session: false,
    },
    Row {
        group: Group::ToAddress,
        repository: "braidonw/zreview",
        number: 9,
        title: "Return the stored outcome from draft_edited",
        author: "braidonw",
        updated: "5h",
        checks: Checks::Passing,
        review: ReviewStatus::ChangesRequested,
        drafts: 0,
        hidden_session: false,
    },
    Row {
        group: Group::ToAddress,
        repository: "acme/widgets",
        number: 401,
        title: "Expose customer timezone on the account API",
        author: "braidonw",
        updated: "2d",
        checks: Checks::Passing,
        review: ReviewStatus::None,
        drafts: 1,
        hidden_session: false,
    },
    Row {
        group: Group::Waiting,
        repository: "braidonw/zreview",
        number: 12,
        title: "Home: settings file and repositories panel",
        author: "braidonw",
        updated: "40m",
        checks: Checks::Pending,
        review: ReviewStatus::None,
        drafts: 0,
        hidden_session: false,
    },
    Row {
        group: Group::Waiting,
        repository: "acme/widgets",
        number: 415,
        title: "Bump tokio to 1.47",
        author: "braidonw",
        updated: "6h",
        checks: Checks::Passing,
        review: ReviewStatus::Approved,
        drafts: 0,
        hidden_session: false,
    },
    Row {
        group: Group::Waiting,
        repository: "acme/widgets",
        number: 388,
        title: "Document the webhook signing scheme",
        author: "braidonw",
        updated: "4d",
        checks: Checks::None,
        review: ReviewStatus::None,
        drafts: 0,
        hidden_session: false,
    },
];

struct HomePrototype {
    focus_handle: FocusHandle,
    variant: Variant,
    cursor: usize,
    repositories_empty: bool,
    hidden_session: bool,
    empty_group: bool,
}

impl HomePrototype {
    fn new(variant: Variant, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            variant,
            cursor: 0,
            repositories_empty: false,
            hidden_session: true,
            empty_group: false,
        }
    }

    fn rows(&self) -> Vec<&'static Row> {
        ROWS.iter()
            .filter(|row| !(self.empty_group && row.group == Group::ToAddress))
            .collect()
    }

    fn rows_in(&self, group: Group) -> Vec<(usize, &'static Row)> {
        self.rows()
            .into_iter()
            .enumerate()
            .filter(|(_, row)| row.group == group)
            .collect()
    }

    fn repositories(&self) -> &'static [Repository] {
        if self.repositories_empty {
            &[]
        } else {
            &REPOSITORIES
        }
    }

    fn next_variant(&mut self, _: &NextVariant, _: &mut Window, cx: &mut Context<Self>) {
        self.variant = self.variant.next();
        cx.notify();
    }

    fn previous_variant(&mut self, _: &PreviousVariant, _: &mut Window, cx: &mut Context<Self>) {
        self.variant = self.variant.previous();
        cx.notify();
    }

    fn cursor_down(&mut self, _: &CursorDown, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.rows().len();
        if count > 0 {
            self.cursor = (self.cursor + 1).min(count - 1);
        }
        cx.notify();
    }

    fn cursor_up(&mut self, _: &CursorUp, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.cursor.saturating_sub(1);
        cx.notify();
    }

    fn toggle_repositories(&mut self, _: &ToggleRepositories, _: &mut Window, cx: &mut Context<Self>) {
        self.repositories_empty = !self.repositories_empty;
        cx.notify();
    }

    fn toggle_hidden_session(&mut self, _: &ToggleHiddenSession, _: &mut Window, cx: &mut Context<Self>) {
        self.hidden_session = !self.hidden_session;
        cx.notify();
    }

    fn toggle_empty_group(&mut self, _: &ToggleEmptyGroup, _: &mut Window, cx: &mut Context<Self>) {
        self.empty_group = !self.empty_group;
        self.cursor = 0;
        cx.notify();
    }

    fn hidden_session_row(&self) -> Option<&'static Row> {
        self.hidden_session.then(|| ROWS.iter().find(|row| row.hidden_session)).flatten()
    }
}

// Shared atoms. Layouts below are free to disagree about everything else.

fn mono() -> gpui::Div {
    div().font_family(theme::font::MONO).text_size(px(theme::size::META))
}

fn sans() -> gpui::Div {
    div().font_family(theme::font::SANS).text_size(px(theme::size::BODY))
}

fn keycap(key: &'static str) -> gpui::Div {
    div()
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(theme::border::DEFAULT))
        .bg(rgb(theme::surface::OVERLAY))
        .font_family(theme::font::MONO)
        .text_size(px(theme::size::LABEL))
        .text_color(rgb(theme::text::TERTIARY))
        .child(key)
}

fn section_label(text: impl Into<SharedString>, count: usize) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .font_family(theme::font::SANS)
                .text_size(px(theme::size::LABEL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::text::TERTIARY))
                .child(text.into().to_uppercase()),
        )
        .child(
            mono()
                .text_size(px(theme::size::LABEL))
                .text_color(rgb(theme::text::FAINT))
                .child(format!("{count}")),
        )
}

fn drafts_badge(count: usize) -> Option<gpui::Div> {
    (count > 0).then(|| {
        div()
            .px_1p5()
            .rounded_sm()
            .bg(rgb(theme::accent::DIM))
            .font_family(theme::font::MONO)
            .text_size(px(theme::size::LABEL))
            .text_color(rgb(theme::accent::TEXT))
            .child(if count == 1 {
                "1 draft".to_owned()
            } else {
                format!("{count} drafts")
            })
    })
}

fn checks_status(checks: Checks) -> Option<gpui::Div> {
    let (text, colour) = match checks {
        Checks::Passing => ("checks passing", theme::severity::SUCCESS_TEXT),
        Checks::Failing => ("checks failing", theme::severity::ERROR_TEXT),
        Checks::Pending => ("checks running", theme::severity::WARNING_TEXT),
        Checks::None => return None,
    };
    Some(mono().text_color(rgb(colour)).child(text))
}

fn review_status(review: ReviewStatus) -> Option<gpui::Div> {
    let (text, colour) = match review {
        ReviewStatus::None => return None,
        ReviewStatus::ReviewedThisHead => ("you reviewed this head", theme::text::TERTIARY),
        ReviewStatus::ChangesRequested => ("changes requested", theme::severity::ERROR_TEXT),
        ReviewStatus::Approved => ("approved", theme::severity::SUCCESS_TEXT),
    };
    Some(mono().text_color(rgb(colour)).child(text))
}

fn identity(row: &Row) -> gpui::Div {
    mono()
        .text_color(rgb(theme::text::SECONDARY))
        .whitespace_nowrap()
        .child(format!("{}#{}", row.repository, row.number))
}

fn refreshed_stamp() -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            mono()
                .text_color(rgb(theme::text::TERTIARY))
                .child("Refreshed 2 min ago"),
        )
        .child(keycap("r"))
}

fn secondary_button(label: &'static str) -> gpui::Div {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(theme::surface::OVERLAY))
        .border_1()
        .border_color(rgb(theme::border::DEFAULT))
        .font_family(theme::font::SANS)
        .text_size(px(theme::size::META))
        .text_color(rgb(theme::text::PRIMARY))
        .cursor_pointer()
        .child(label)
}

fn primary_button(label: &'static str) -> gpui::Div {
    div()
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(theme::accent::BASE))
        .font_family(theme::font::SANS)
        .text_size(px(theme::size::META))
        .text_color(rgb(theme::text::ON_ACCENT))
        .cursor_pointer()
        .child(label)
}

fn repository_state_line(repository: &Repository) -> gpui::Div {
    match repository.state {
        RepositoryState::Loaded(count) => mono()
            .text_color(rgb(theme::text::TERTIARY))
            .child(format!("{count} open")),
        RepositoryState::Loading => mono()
            .text_color(rgb(theme::accent::TEXT))
            .child("loading..."),
        RepositoryState::Failed(message) => mono()
            .text_color(rgb(theme::severity::ERROR_TEXT))
            .overflow_hidden()
            .whitespace_nowrap()
            .child(message),
    }
}

fn empty_repositories(copy: &'static str) -> gpui::Div {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            sans()
                .text_size(px(theme::size::HEADING))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(theme::text::PRIMARY))
                .child("No repositories yet"),
        )
        .child(sans().text_color(rgb(theme::text::SECONDARY)).child(copy))
        .child(primary_button("Add repository..."))
}

impl HomePrototype {
    fn row_click(&self, index: usize, cx: &mut Context<Self>) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
        let this = cx.entity();
        move |_, _, cx| {
            this.update(cx, |this, cx| {
                this.cursor = index;
                cx.notify();
            });
        }
    }

    // Variant A. One dense ledger, single-line rows, repositories as a footer.

    fn render_ledger(&self, cx: &mut Context<Self>) -> gpui::Div {
        let header = div()
            .flex_shrink_0()
            .h(px(52.0))
            .px_5()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(rgb(theme::border::DEFAULT))
            .bg(rgb(theme::surface::RAISED))
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .child(
                        sans()
                            .text_size(px(theme::size::HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::text::PRIMARY))
                            .child("Home"),
                    )
                    .child(
                        mono()
                            .text_color(rgb(theme::text::TERTIARY))
                            .child(format!("{} pull requests across {} repositories", self.rows().len(), self.repositories().len())),
                    ),
            )
            .child(refreshed_stamp());

        let body = if self.repositories().is_empty() {
            empty_repositories("Add a local clone and Home lists the pull requests that want you.").into_any()
        } else {
            let mut body = div().id("ledger-body").flex_1().min_h_0().overflow_y_scroll().px_5().pb_6();
            for group in Group::ALL {
                let rows = self.rows_in(group);
                let mut section = div()
                    .pt_5()
                    .flex()
                    .flex_col()
                    .child(div().pb_2().child(section_label(group.title(), rows.len())));
                if rows.is_empty() {
                    section = section.child(
                        div()
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .child(sans().text_color(rgb(theme::text::FAINT)).child(group.empty_copy())),
                    );
                }
                for (index, row) in rows {
                    let selected = index == self.cursor;
                    let live = self.hidden_session && row.hidden_session;
                    section = section.child(
                        div()
                            .id(("ledger-row", index))
                            .h(px(36.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .rounded_sm()
                            .cursor_pointer()
                            .bg(if selected { rgb(theme::surface::SELECTED) } else { rgb(theme::surface::BASE) })
                            .hover(|style| style.bg(rgb(theme::surface::HOVER)))
                            .on_mouse_down(MouseButton::Left, self.row_click(index, cx))
                            .child(
                                sans()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(rgb(theme::text::PRIMARY))
                                    .child(row.title),
                            )
                            .when(live, |r| {
                                r.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .px_1p5()
                                        .rounded_sm()
                                        .border_1()
                                        .border_color(rgb(theme::accent::BASE))
                                        .child(mono().text_size(px(theme::size::LABEL)).text_color(rgb(theme::accent::TEXT)).child("open · review running"))
                                        .child(keycap("cmd-[")),
                                )
                            })
                            .children(drafts_badge(row.drafts))
                            .children(review_status(row.review))
                            .children(checks_status(row.checks))
                            .child(div().w(px(200.0)).flex().justify_end().child(identity(row)))
                            .child(mono().w(px(70.0)).text_color(rgb(theme::text::TERTIARY)).overflow_hidden().whitespace_nowrap().child(row.author))
                            .child(mono().w(px(32.0)).text_color(rgb(theme::text::TERTIARY)).child(row.updated)),
                    );
                }
                body = body.child(section);
            }
            body.into_any()
        };

        let mut footer = div()
            .flex_shrink_0()
            .border_t_1()
            .border_color(rgb(theme::border::DEFAULT))
            .bg(rgb(theme::surface::RAISED))
            .px_5()
            .py_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_label("Repositories", self.repositories().len()))
                    .child(secondary_button("Add...")),
            );
        if self.repositories().is_empty() {
            footer = footer.child(sans().text_color(rgb(theme::text::TERTIARY)).child("None configured. Settings live in ~/.config/zreview/settings.toml."));
        }
        for repository in self.repositories() {
            footer = footer.child(
                div()
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(mono().w(px(200.0)).text_color(rgb(theme::text::SECONDARY)).child(repository.slug))
                    .child(mono().w(px(220.0)).text_color(rgb(theme::text::FAINT)).child(repository.path))
                    .child(div().flex_1().min_w_0().child(repository_state_line(repository)))
                    .child(mono().text_color(rgb(theme::text::FAINT)).cursor_pointer().child("Remove")),
            );
        }

        div().size_full().flex().flex_col().child(header).child(body).child(footer)
    }

    // Variant B. Session-shaped: repositories own the left sidebar, the list is
    // two-line rows, the hidden Session is a banner.

    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut sidebar = div()
            .w(px(290.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(theme::border::DEFAULT))
            .bg(rgb(theme::surface::BASE))
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .py_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(rgb(theme::border::DEFAULT))
                    .child(
                        sans()
                            .text_size(px(theme::size::HEADING))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme::text::PRIMARY))
                            .child("Repositories"),
                    )
                    .child(secondary_button("Add...")),
            );
        if self.repositories().is_empty() {
            sidebar = sidebar.child(
                div()
                    .px_3()
                    .py_5()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(sans().text_color(rgb(theme::text::SECONDARY)).child("No repositories yet."))
                    .child(sans().text_size(px(theme::size::META)).text_color(rgb(theme::text::TERTIARY)).child("Add a local clone with a GitHub remote. Home lists its open pull requests."))
                    .child(div().pt_2().child(primary_button("Add repository..."))),
            );
        }
        for repository in self.repositories() {
            sidebar = sidebar.child(
                div()
                    .h(px(48.0))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(theme::border::SUBTLE))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(mono().text_color(rgb(theme::text::PRIMARY)).child(repository.slug))
                            .child(repository_state_line(repository)),
                    )
                    .child(mono().text_color(rgb(theme::text::FAINT)).cursor_pointer().child("×")),
            );
        }
        sidebar = sidebar.child(div().flex_1()).child(
            div()
                .flex_shrink_0()
                .px_3()
                .py_3()
                .border_t_1()
                .border_color(rgb(theme::border::DEFAULT))
                .child(refreshed_stamp()),
        );

        let mut main = div().flex_1().min_w_0().h_full().flex().flex_col();
        if self.repositories().is_empty() {
            main = main.child(empty_repositories("Once a repository is added, its pull requests are grouped by what they want from you."));
        } else {
            if let Some(row) = self.hidden_session_row() {
                main = main.child(
                    div()
                        .flex_shrink_0()
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_3()
                        .border_b_1()
                        .border_color(rgb(theme::border::DEFAULT))
                        .bg(rgb(theme::accent::DIM))
                        .child(mono().text_color(rgb(theme::accent::TEXT)).child(format!("{}#{} is open behind Home. Review running.", row.repository, row.number)))
                        .child(div().flex_1())
                        .child(mono().text_color(rgb(theme::accent::TEXT)).child("Return"))
                        .child(keycap("cmd-[")),
                );
            }
            let mut list = div().id("sidebar-body").flex_1().min_h_0().overflow_y_scroll().pb_6();
            for group in Group::ALL {
                let rows = self.rows_in(group);
                let mut section = div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_4()
                            .pt_5()
                            .pb_2()
                            .child(section_label(group.title(), rows.len())),
                    );
                if rows.is_empty() {
                    section = section.child(div().px_4().py_2().child(sans().text_color(rgb(theme::text::FAINT)).child(group.empty_copy())));
                }
                for (index, row) in rows {
                    let selected = index == self.cursor;
                    section = section.child(
                        div()
                            .id(("sidebar-row", index))
                            .h(px(52.0))
                            .px_4()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap_1()
                            .cursor_pointer()
                            .border_l_2()
                            .border_color(if selected { rgb(theme::accent::BASE) } else { rgb(theme::surface::BASE) })
                            .bg(if selected { rgb(theme::surface::SELECTED) } else { rgb(theme::surface::BASE) })
                            .hover(|style| style.bg(rgb(theme::surface::HOVER)))
                            .on_mouse_down(MouseButton::Left, self.row_click(index, cx))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        sans()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_color(rgb(theme::text::PRIMARY))
                                            .child(row.title),
                                    )
                                    .children(drafts_badge(row.drafts)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(identity(row))
                                    .child(mono().text_color(rgb(theme::text::TERTIARY)).child(row.author))
                                    .child(mono().text_color(rgb(theme::text::TERTIARY)).child(row.updated))
                                    .children(checks_status(row.checks))
                                    .children(review_status(row.review)),
                            ),
                    );
                }
                list = list.child(section);
            }
            main = main.child(list);
        }

        div().size_full().flex().child(sidebar).child(main)
    }

    // Variant C. Three columns of cards, repositories as chips in a toolbar.

    fn render_columns(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut chips = div().flex().items_center().gap_2().flex_wrap();
        for repository in self.repositories() {
            let (bg, fg, suffix) = match repository.state {
                RepositoryState::Loaded(_) => (theme::surface::OVERLAY, theme::text::SECONDARY, ""),
                RepositoryState::Loading => (theme::surface::OVERLAY, theme::text::FAINT, " ..."),
                RepositoryState::Failed(_) => (theme::severity::ERROR_DIM, theme::severity::ERROR_TEXT, " !"),
            };
            chips = chips.child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(rgb(bg))
                    .child(mono().text_color(rgb(fg)).child(format!("{}{suffix}", repository.slug))),
            );
        }
        chips = chips.child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(rgb(theme::border::STRONG))
                .cursor_pointer()
                .child(mono().text_color(rgb(theme::text::TERTIARY)).child("+ Add")),
        );

        let toolbar = div()
            .flex_shrink_0()
            .px_5()
            .py_3()
            .flex()
            .items_center()
            .gap_4()
            .border_b_1()
            .border_color(rgb(theme::border::DEFAULT))
            .bg(rgb(theme::surface::RAISED))
            .child(
                sans()
                    .text_size(px(theme::size::HEADING))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme::text::PRIMARY))
                    .child("Home"),
            )
            .child(div().flex_1().min_w_0().child(chips))
            .child(refreshed_stamp());

        let failures = self
            .repositories()
            .iter()
            .filter_map(|repository| match repository.state {
                RepositoryState::Failed(message) => Some((repository.slug, message)),
                RepositoryState::Loaded(_) | RepositoryState::Loading => None,
            })
            .map(|(slug, message)| {
                div()
                    .flex_shrink_0()
                    .px_5()
                    .py_1p5()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(rgb(theme::severity::ERROR_DIM))
                    .child(mono().text_color(rgb(theme::severity::ERROR_TEXT)).child(format!("{slug}: {message}")))
                    .child(div().flex_1())
                    .child(mono().text_color(rgb(theme::severity::ERROR_TEXT)).cursor_pointer().child("Retry"))
                    .child(mono().text_color(rgb(theme::severity::ERROR_TEXT)).cursor_pointer().child("Remove"))
            })
            .collect::<Vec<_>>();

        let body = if self.repositories().is_empty() {
            empty_repositories("Three columns fill in as pull requests arrive: To review, To address, Waiting on others.").into_any()
        } else {
            let mut columns = div().flex_1().min_h_0().px_5().pt_5().pb_6().flex().gap_4();
            for group in Group::ALL {
                let rows = self.rows_in(group);
                let mut column = div()
                    .id(SharedString::from(format!("column-{}", group.title())))
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().pb_1().child(section_label(group.title(), rows.len())));
                if rows.is_empty() {
                    column = column.child(
                        div()
                            .p_4()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(theme::border::SUBTLE))
                            .child(sans().text_color(rgb(theme::text::FAINT)).child(group.empty_copy())),
                    );
                }
                for (index, row) in rows {
                    let selected = index == self.cursor;
                    let live = self.hidden_session && row.hidden_session;
                    let mut card = div()
                        .id(("card", index))
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(if selected { rgb(theme::border::FOCUS) } else { rgb(theme::border::SUBTLE) })
                        .bg(if selected { rgb(theme::surface::SELECTED) } else { rgb(theme::surface::RAISED) })
                        .hover(|style| style.bg(rgb(theme::surface::HOVER)))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, self.row_click(index, cx))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(sans().text_color(rgb(theme::text::PRIMARY)).line_height(px(18.0)).child(row.title))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(identity(row))
                                .child(mono().text_color(rgb(theme::text::TERTIARY)).child(row.author))
                                .child(div().flex_1())
                                .child(mono().text_color(rgb(theme::text::TERTIARY)).child(row.updated)),
                        );
                    let statuses = [checks_status(row.checks), review_status(row.review), drafts_badge(row.drafts)]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>();
                    if !statuses.is_empty() {
                        card = card.child(div().flex().items_center().gap_3().flex_wrap().children(statuses));
                    }
                    if live {
                        card = card.child(
                            div()
                                .pt_1()
                                .border_t_1()
                                .border_color(rgb(theme::border::SUBTLE))
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(rgb(theme::accent::BASE)))
                                .child(mono().text_color(rgb(theme::accent::TEXT)).child("Open behind Home. Review running."))
                                .child(div().flex_1())
                                .child(keycap("cmd-[")),
                        );
                    }
                    column = column.child(card);
                }
                columns = columns.child(column);
            }
            columns.into_any()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .children(failures)
            .child(body)
    }

    fn render_switcher(&self, cx: &mut Context<Self>) -> gpui::Div {
        let previous = cx.entity();
        let next = cx.entity();
        div()
            .absolute()
            .bottom(px(16.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px_4()
                    .py_2()
                    .rounded_full()
                    .bg(rgb(0xf4f4f0))
                    .flex()
                    .items_center()
                    .gap_4()
                    .font_family(theme::font::MONO)
                    .text_size(px(12.0))
                    .text_color(rgb(0x111111))
                    .child(
                        div()
                            .id("previous-variant")
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                previous.update(cx, |this, cx| {
                                    this.variant = this.variant.previous();
                                    cx.notify();
                                });
                            })
                            .child("◀"),
                    )
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("{} · {}", self.variant.key(), self.variant.name())))
                    .child(
                        div()
                            .id("next-variant")
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                next.update(cx, |this, cx| {
                                    this.variant = this.variant.next();
                                    cx.notify();
                                });
                            })
                            .child("▶"),
                    )
                    .child(div().text_color(rgb(0x777777)).child("j/k cursor · e repositories · s session · g empty group")),
            )
    }
}

impl Focusable for HomePrototype {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HomePrototype {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.variant {
            Variant::Ledger => self.render_ledger(cx),
            Variant::Sidebar => self.render_sidebar(cx),
            Variant::Columns => self.render_columns(cx),
        };
        div()
            .id("home-prototype")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::next_variant))
            .on_action(cx.listener(Self::previous_variant))
            .on_action(cx.listener(Self::cursor_down))
            .on_action(cx.listener(Self::cursor_up))
            .on_action(cx.listener(Self::toggle_repositories))
            .on_action(cx.listener(Self::toggle_hidden_session))
            .on_action(cx.listener(Self::toggle_empty_group))
            .relative()
            .size_full()
            .bg(rgb(theme::surface::BASE))
            .child(content)
            .child(self.render_switcher(cx))
    }
}

fn main() {
    let variant = match env::args().nth(1).as_deref() {
        Some("b" | "B") => Variant::Sidebar,
        Some("c" | "C") => Variant::Columns,
        _ => Variant::Ledger,
    };

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("right", NextVariant, Some(CONTEXT)),
            KeyBinding::new("left", PreviousVariant, Some(CONTEXT)),
            KeyBinding::new("j", CursorDown, Some(CONTEXT)),
            KeyBinding::new("down", CursorDown, Some(CONTEXT)),
            KeyBinding::new("k", CursorUp, Some(CONTEXT)),
            KeyBinding::new("up", CursorUp, Some(CONTEXT)),
            KeyBinding::new("e", ToggleRepositories, Some(CONTEXT)),
            KeyBinding::new("s", ToggleHiddenSession, Some(CONTEXT)),
            KeyBinding::new("g", ToggleEmptyGroup, Some(CONTEXT)),
        ]);

        let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title("ZReview · Home prototype");
                let view = cx.new(|cx| HomePrototype::new(variant, cx));
                let focus_handle = view.read(cx).focus_handle.clone();
                window.focus(&focus_handle);
                view
            },
        )
        .expect("failed to open the prototype window");
        cx.activate(true);
    });
}
