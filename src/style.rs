use std::io::IsTerminal;

/// ANSI styling that turns itself off when nobody is watching.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub enabled: bool,
}

impl Paint {
    pub fn new(no_color_flag: bool) -> Self {
        let disabled = no_color_flag
            || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
            || std::env::var("TERM").is_ok_and(|t| t == "dumb")
            || !std::io::stdout().is_terminal();
        Paint { enabled: !disabled }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if !self.enabled || s.is_empty() {
            s.to_string()
        } else {
            format!("\x1b[{code}m{s}\x1b[0m")
        }
    }

    /// Faint text: metadata, rules, counts.
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }

    pub fn bold(&self, s: &str) -> String {
        self.wrap("1", s)
    }

    /// Timestamps.
    pub fn date(&self, s: &str) -> String {
        self.wrap("36", s)
    }

    /// Entry numbers.
    pub fn index(&self, s: &str) -> String {
        self.wrap("2", s)
    }

    /// #tags and @mentions.
    pub fn tag(&self, s: &str) -> String {
        self.wrap("33", s)
    }

    /// A search hit.
    pub fn hit(&self, s: &str) -> String {
        self.wrap("1;33", s)
    }

    /// The search hit the reader is currently sitting on.
    pub fn focus(&self, s: &str) -> String {
        self.wrap("1;30;43", s)
    }

    /// Structural dividers: the entry separators in the reader.
    pub fn separator(&self, s: &str) -> String {
        self.wrap("36", s)
    }

    /// Reverse video, for status bars and panels.
    pub fn reverse(&self, s: &str) -> String {
        self.wrap("7", s)
    }

    pub fn ok(&self, s: &str) -> String {
        self.wrap("32", s)
    }

    pub fn warn(&self, s: &str) -> String {
        self.wrap("1;31", s)
    }
}
