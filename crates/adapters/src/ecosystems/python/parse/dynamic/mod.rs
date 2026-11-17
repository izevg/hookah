//! Output of the tools that run code: pytest and mutmut.

mod mutmut;
mod pytest;

pub use mutmut::parse as mutmut;
pub use pytest::parse as pytest;
