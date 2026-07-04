//! HTML escaping — re-exported from `tumult_core::report`, which owns the
//! shared implementation (the XML variant lives there too, used by the
//! shared `JUnit` renderer).

pub(crate) use tumult_core::report::html_escape;
