//! Node (TypeScript / JavaScript) toolchain detection.
//!
//! Reads `package.json` scripts and dependencies plus tool configs to find the formatter
//! (`biome`, `prettier`), linter (`biome`, `eslint`), and test runner (`vitest`, `jest`).
//! Resolves binaries from the local `node_modules/.bin` through the detected package manager.
