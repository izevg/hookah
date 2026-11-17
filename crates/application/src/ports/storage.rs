//! Storage ports. Post-MVP: no traits are defined yet.
//!
//! The MVP keeps no state between hook calls. Later this module will hold `BaselineStore` (the
//! finding baseline per project and file) and, if needed, a session store.
