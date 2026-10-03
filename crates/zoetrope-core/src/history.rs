//! Undo/redo. A `Document` is a project plus its edit history; it is the only
//! sanctioned way to mutate a project after load.

use crate::edit::Edit;
use crate::error::Result;
use crate::model::Project;

/// A labelled group of edits that is undone/redone as one unit.
#[derive(Debug, Clone)]
struct Transaction {
    label: String,
    /// Identifies the project state *after* this transaction (for dirty tracking).
    state: u64,
    /// Edits that reverse this transaction, in the order they must be applied.
    undo: Vec<Edit>,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub project: Project,
    undo_stack: Vec<Transaction>,
    redo_stack: Vec<Transaction>,
    next_state: u64,
    saved_state: u64,
}

const BASE_STATE: u64 = 0;

impl Document {
    pub fn new(project: Project) -> Self {
        Document { project, undo_stack: Vec::new(), redo_stack: Vec::new(), next_state: BASE_STATE + 1, saved_state: BASE_STATE }
    }

    /// Applies `edits` atomically as one undoable step. If any edit fails,
    /// the already-applied ones are rolled back and the error is returned.
    pub fn execute(&mut self, label: impl Into<String>, edits: Vec<Edit>) -> Result<()> {
        if edits.is_empty() {
            return Ok(());
        }
        let undo = apply_all(&mut self.project, edits)?;
        let state = self.next_state;
        self.next_state += 1;
        self.undo_stack.push(Transaction { label: label.into(), state, undo });
        self.redo_stack.clear();
        Ok(())
    }

    /// Returns `Ok(false)` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<bool> {
        let Some(t) = self.undo_stack.pop() else {
            return Ok(false);
        };
        match apply_all(&mut self.project, t.undo.clone()) {
            Ok(redo) => {
                self.redo_stack.push(Transaction { undo: redo, ..t });
                Ok(true)
            }
            Err(e) => {
                self.undo_stack.push(t);
                Err(e)
            }
        }
    }

    /// Returns `Ok(false)` when there is nothing to redo.
    pub fn redo(&mut self) -> Result<bool> {
        let Some(t) = self.redo_stack.pop() else {
            return Ok(false);
        };
        match apply_all(&mut self.project, t.undo.clone()) {
            Ok(undo) => {
                self.undo_stack.push(Transaction { undo, ..t });
                Ok(true)
            }
            Err(e) => {
                self.redo_stack.push(t);
                Err(e)
            }
        }
    }

    /// Undoes the last step and drops it from history (no redo), e.g. to
    /// abandon an object that was created and then left empty.
    pub fn undo_discard(&mut self) -> Result<bool> {
        let undone = self.undo()?;
        if undone {
            self.redo_stack.pop();
        }
        Ok(undone)
    }

    /// Steps that can be undone / redone.
    pub fn undo_depth(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo_stack.last().map(|t| t.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo_stack.last().map(|t| t.label.as_str())
    }

    fn current_state(&self) -> u64 {
        self.undo_stack.last().map_or(BASE_STATE, |t| t.state)
    }

    pub fn mark_saved(&mut self) {
        self.saved_state = self.current_state();
    }

    /// Treats the current state as unsaved (e.g. work recovered from an
    /// autosave) until the next `mark_saved`.
    pub fn mark_unsaved(&mut self) {
        self.saved_state = u64::MAX;
    }

    /// True when the project differs from the last saved (or loaded) state.
    pub fn is_dirty(&self) -> bool {
        self.current_state() != self.saved_state
    }
}

/// Applies edits in order; returns the inverse sequence (already in the order
/// it must be applied). On failure rolls back and leaves `p` unchanged.
fn apply_all(p: &mut Project, edits: Vec<Edit>) -> Result<Vec<Edit>> {
    let mut inverses = Vec::with_capacity(edits.len());
    for edit in edits {
        match edit.apply(p) {
            Ok(inv) => inverses.push(inv),
            Err(e) => {
                for inv in inverses.into_iter().rev() {
                    inv.apply(p).expect("inverse of a just-applied edit must apply");
                }
                return Err(e);
            }
        }
    }
    inverses.reverse();
    Ok(inverses)
}
