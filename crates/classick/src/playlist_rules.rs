//! Smart-playlist rule evaluation.
//!
//! Task 2 fills this — declarative match rules (artist/album/genre/year,
//! is/contains/gte/lte), limits, and ordering, evaluated host-side against
//! the library index at sync/preview time. This task only needs the type to
//! exist so `playlist::SmartPlaylist` compiles standalone.

use serde::{Deserialize, Serialize};

// Task 2 fills this.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SmartRules;
