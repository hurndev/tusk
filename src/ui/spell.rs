//! Spellchecking as the full-screen views see it: off unless asked for.
//!
//! The state is deliberately *not* remembered. Every editor or reader session
//! starts with spellchecking off, and nothing is read from disk until the
//! writer presses F2 — so the dictionary load is paid once, on demand, rather
//! than on every start.

use crate::config::Config;
use crate::spell;

use super::frame::Flash;

/// Spellchecking state for one session.
pub struct Spell {
    on: bool,
    dict: Option<spell::Dictionary>,
    cfg: Config,
}

impl Spell {
    /// Always starts off, whatever the config says.
    pub fn new(cfg: &Config) -> Spell {
        Spell {
            on: false,
            dict: None,
            cfg: cfg.clone(),
        }
    }

    /// The dictionary, when spellchecking has been turned on.
    pub fn active(&mut self) -> Option<&mut spell::Dictionary> {
        if self.on { self.dict.as_mut() } else { None }
    }

    fn label(&self) -> String {
        match &self.dict {
            Some(dict) => dict.summary(),
            None => "no dictionary".to_string(),
        }
    }

    /// Turn it on or off. The first turn-on loads the dictionary; if there
    /// isn't one, it stays off and says why.
    pub fn toggle(&mut self) -> Flash {
        if self.on {
            self.on = false;
            return Flash::new("spellcheck off");
        }
        if self.dict.is_none() {
            match spell::Dictionary::load(&self.cfg) {
                spell::Status::Ready(dict) => self.dict = Some(*dict),
                spell::Status::Disabled(reason) => return Flash::error(reason),
            }
        }
        self.on = true;
        Flash::new(format!("spellcheck on · {}", self.label()))
    }

    /// Accept the word under the cursor, and remember it.
    pub fn add_word(&mut self, line: &str, col: usize) -> Flash {
        let Some(word) = spell::word_at(line, col) else {
            return Flash::error("no word under the cursor");
        };
        let text = line[word.bytes.0..word.bytes.1].to_string();
        let Some(dict) = self.active() else {
            return Flash::error("spellcheck is off — F2 turns it on");
        };
        match dict.add_word(&text) {
            Ok(true) => Flash::new(format!("added \"{text}\" to your words")),
            Ok(false) => Flash::new(format!("\"{text}\" is already in your words")),
            Err(err) => Flash::error(format!("could not save \"{text}\": {err}")),
        }
    }
}
