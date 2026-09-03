//! Which screen the window shows, and the one Session it holds.

/// Which pull request a Session is open on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PullRequestId {
    /// `owner/name`, as GitHub names the repository.
    pub repository: String,
    pub number: u64,
}

impl PullRequestId {
    /// `owner/name#number`, which is how a row and the header slot name it.
    ///
    /// The one place that format is written, so a row and the Session opened
    /// from it can never name the same pull request differently.
    pub(crate) fn identity(&self) -> String {
        format!("{}#{}", self.repository, self.number)
    }

    /// Whether both name one pull request, which GitHub decides without regard
    /// to case, so nothing here may either.
    pub(crate) fn is_same_pull_request(&self, other: &Self) -> bool {
        self.number == other.number && self.repository.eq_ignore_ascii_case(&other.repository)
    }
}

/// The one Session a window holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenSession {
    /// Opened from a Home row, which is what puts a Home behind it.
    FromRow(PullRequestId),
    /// Opened by the command line, with no Home behind it at all.
    FromCommandLine,
}

impl OpenSession {
    /// The pull request this Session is open on, absent for one the command
    /// line opened on something Home never listed.
    #[must_use]
    pub const fn pull_request(&self) -> Option<&PullRequestId> {
        match self {
            Self::FromRow(pull_request) => Some(pull_request),
            Self::FromCommandLine => None,
        }
    }

    /// `owner/name#number` of the row this Session was opened from.
    ///
    /// Its presence is what says there is a Home behind this Session.
    #[must_use]
    pub fn row_identity(&self) -> Option<String> {
        self.pull_request().map(PullRequestId::identity)
    }
}

/// Which screen the window shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Showing {
    Home,
    Session,
}

/// What opening a row asked of the Session already alive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum Opened {
    /// The Session already alive is this pull request's own, so it is shown
    /// again rather than loaded a second time.
    Returned,
    /// Nothing alive was this pull request's, so one is to be loaded.
    Loading,
    /// This window has no Home to open a row from, so nothing was opened. Only
    /// the command line opens such a window, and it never lists a row.
    Refused,
}

/// Which screen the window shows, and the one Session it holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionSlot {
    session: Option<OpenSession>,
    showing: Showing,
}

impl SessionSlot {
    /// A window that opened on Home, holding no Session yet.
    #[must_use]
    pub const fn home() -> Self {
        Self {
            session: None,
            showing: Showing::Home,
        }
    }

    /// A window the command line opened straight into a Session.
    ///
    /// There is no Home behind it, so this window never shows one.
    #[must_use]
    pub const fn command_line() -> Self {
        Self {
            session: Some(OpenSession::FromCommandLine),
            showing: Showing::Session,
        }
    }

    #[must_use]
    pub const fn showing(&self) -> Showing {
        self.showing
    }

    /// The Session this window holds, in front of Home or alive behind it.
    #[must_use]
    pub const fn session(&self) -> Option<&OpenSession> {
        self.session.as_ref()
    }

    /// Opens `pull_request` from a row, showing its Session.
    ///
    /// The Session already alive on this pull request is shown again rather
    /// than loaded a second time. Any other is dropped, silently, because
    /// Drafts already persist. A window with no Home refuses the row instead,
    /// rather than growing a Home it never had.
    pub fn open(&mut self, pull_request: PullRequestId) -> Opened {
        match &self.session {
            Some(OpenSession::FromCommandLine) => return Opened::Refused,
            Some(OpenSession::FromRow(open)) if open.is_same_pull_request(&pull_request) => {
                self.showing = Showing::Session;
                return Opened::Returned;
            }
            Some(OpenSession::FromRow(_)) | None => {}
        }
        self.showing = Showing::Session;
        self.session = Some(OpenSession::FromRow(pull_request));
        Opened::Loading
    }

    /// Shows the Session alive behind Home again, exactly as it was left.
    ///
    /// Answers `false` when no Session is alive, which the header slot's own
    /// absence already keeps a reviewer from asking for.
    #[must_use]
    pub fn return_to_session(&mut self) -> bool {
        if self.session.is_none() {
            return false;
        }
        self.showing = Showing::Session;
        true
    }

    /// Shows Home again, leaving the Session alive behind it.
    ///
    /// Answers `false` for a Session the command line opened, which has no
    /// Home behind it to go back to, and for a window already showing Home.
    #[must_use]
    pub fn back_to_home(&mut self) -> bool {
        match &self.session {
            Some(OpenSession::FromRow(_)) if self.showing == Showing::Session => {
                self.showing = Showing::Home;
                true
            }
            Some(OpenSession::FromRow(_) | OpenSession::FromCommandLine) | None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull_request(number: u64) -> PullRequestId {
        PullRequestId {
            repository: "acme/widgets".to_owned(),
            number,
        }
    }

    #[test]
    fn a_window_that_opened_on_home_holds_no_session() {
        let slot = SessionSlot::home();

        assert_eq!(slot.showing(), Showing::Home);
        assert_eq!(slot.session(), None);
    }

    #[test]
    fn opening_a_row_shows_its_session_and_leaves_it_alive() {
        let mut slot = SessionSlot::home();

        assert_eq!(slot.open(pull_request(412)), Opened::Loading);

        assert_eq!(slot.showing(), Showing::Session);
        assert_eq!(
            slot.session(),
            Some(&OpenSession::FromRow(pull_request(412))),
        );
    }

    /// Coming back to the pull request already open costs nothing, which is the
    /// whole point of keeping it alive.
    #[test]
    fn opening_the_pull_request_already_alive_returns_to_it_unloaded() {
        let mut slot = SessionSlot::home();
        let _opened = slot.open(pull_request(412));
        assert!(slot.back_to_home());

        assert_eq!(slot.open(pull_request(412)), Opened::Returned);

        assert_eq!(slot.showing(), Showing::Session);
    }

    /// GitHub compares two repository names without regard to case, so a row
    /// that came back cased differently is still the same pull request.
    #[test]
    fn a_repository_cased_differently_still_names_the_session_already_alive() {
        let mut slot = SessionSlot::home();
        let _opened = slot.open(pull_request(412));

        let differently_cased = PullRequestId {
            repository: "ACME/Widgets".to_owned(),
            number: 412,
        };

        assert_eq!(slot.open(differently_cased), Opened::Returned);
    }

    /// Drafts already persist, so the Session that goes is not carrying
    /// anything the reviewer would have to be asked about.
    #[test]
    fn opening_a_different_pull_request_drops_the_one_that_was_alive() {
        let mut slot = SessionSlot::home();
        let _opened = slot.open(pull_request(412));

        assert_eq!(slot.open(pull_request(398)), Opened::Loading);

        assert_eq!(
            slot.session(),
            Some(&OpenSession::FromRow(pull_request(398))),
            "only the pull request just opened is alive",
        );
    }

    /// Back is a navigation rather than a close, which is what makes returning
    /// to a half-finished review instant.
    #[test]
    fn back_shows_home_and_leaves_the_session_alive_behind_it() {
        let mut slot = SessionSlot::home();
        let _opened = slot.open(pull_request(412));

        assert!(slot.back_to_home());

        assert_eq!(slot.showing(), Showing::Home);
        assert_eq!(
            slot.session(),
            Some(&OpenSession::FromRow(pull_request(412))),
        );
    }

    #[test]
    fn the_header_slot_returns_to_the_session_alive_behind_home() {
        let mut slot = SessionSlot::home();
        let _opened = slot.open(pull_request(412));
        assert!(slot.back_to_home());

        assert!(slot.return_to_session());

        assert_eq!(slot.showing(), Showing::Session);
    }

    #[test]
    fn returning_to_a_session_is_refused_when_none_is_alive() {
        let mut slot = SessionSlot::home();

        assert!(!slot.return_to_session());

        assert_eq!(slot.showing(), Showing::Home);
    }

    /// Only Home lists a row, and a window the command line opened never shows
    /// one, so a row arriving here is a mistake rather than a navigation.
    #[test]
    fn a_window_the_command_line_opened_refuses_to_open_a_row() {
        let mut slot = SessionSlot::command_line();

        assert_eq!(slot.open(pull_request(412)), Opened::Refused);

        assert_eq!(slot.session(), Some(&OpenSession::FromCommandLine));
        assert_eq!(slot.showing(), Showing::Session);
    }

    #[test]
    fn a_session_the_command_line_opened_has_no_home_to_go_back_to() {
        let mut slot = SessionSlot::command_line();

        assert_eq!(slot.showing(), Showing::Session);
        assert_eq!(slot.session(), Some(&OpenSession::FromCommandLine));
        assert!(!slot.back_to_home());
        assert_eq!(slot.showing(), Showing::Session);
    }

    /// The identity is both what the header slot reads and what says there is a
    /// Home behind the Session at all.
    #[test]
    fn only_a_session_opened_from_a_row_carries_the_identity_of_one() {
        assert_eq!(
            OpenSession::FromRow(pull_request(412)).row_identity(),
            Some("acme/widgets#412".to_owned()),
        );
        assert_eq!(OpenSession::FromCommandLine.row_identity(), None);
    }
}
