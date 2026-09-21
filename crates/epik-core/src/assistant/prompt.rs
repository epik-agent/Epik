//! The system prompt, compiled in.

static PROMPT: include_crypt::EncryptedFile = include_crypt::include_crypt!("prompt.md");

/// Epik's voice: the system prompt that leads every turn, whoever the
/// host is — the desktop app today, a headless daemon tomorrow. Written
/// in prompt.md at this crate's root, compiled in encrypted so the binary
/// doesn't hand it to strings(1) — obfuscation, not secrecy — and never
/// part of any transcript. It rides with `native` because every host
/// that speaks is one; the obfuscation machinery doesn't build for wasm.
pub static SYSTEM_PROMPT: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| PROMPT.decrypt_str().expect("the prompt decrypts to utf-8"));

#[cfg(test)]
mod tests {
    use super::*;

    /// The obfuscation round-trips: what ships encrypted decrypts to
    /// exactly the file Bill writes.
    #[test]
    fn the_system_prompt_decrypts_to_the_prompt_file() {
        assert_eq!(*SYSTEM_PROMPT, include_str!("../../prompt.md"));
    }
}
