//! Asking for a password at a terminal: nothing is echoed, and an empty answer (or no
//! terminal to ask on) is no answer.

pub fn read(prompt: &str) -> Option<String> {
    let answer = rpassword::prompt_password(prompt).ok()?;
    (!answer.is_empty()).then_some(answer)
}
