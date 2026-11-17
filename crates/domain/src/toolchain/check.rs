//! `CheckKind`: `Format`, `Lint`, `Test`.
//!
//! Also holds the phase rule: the edit phase runs `Format` and `Lint`, and the stop phase runs
//! `Test`. Formatters run before linters so lint findings match the final text of the file.
