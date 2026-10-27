//! # Manyroot
//!
//! Many minds. Shared ground.
//!
//! The core protocol: humans and agents add immutable, signed contributions to
//! a Ground, and the contributions form a causal graph. Replicas converge by
//! exchanging whatever contributions the other side lacks. Each reader decides
//! what counts as accepted by applying its own validation policy.
//!
//! | Module           | Subsystem | Responsibility                                  |
//! |------------------|-----------|-------------------------------------------------|
//! | [`contribution`] | Seed      | Typed, signed, content-addressed contributions  |
//! | [`graph`]        | Roots     | Causal graph, protocol rules, causal heads      |
//! | [`validation`]   | Proof     | Policies, acceptance, conflicts                 |
//! | [`store`]        | Ground    | A replica on disk, local identities, sync       |
//! | [`bundle`]       | Weave     | Portable contribution streams between replicas  |
//! | [`identity`]     | Stewards  | Agent keys and signatures                       |

pub mod bundle;
pub mod contribution;
pub mod graph;
pub mod id;
pub mod identity;
pub mod store;
pub mod validation;

pub use contribution::{Body, BodyKind, Contribution, RelationKind, Verdict};
pub use graph::{Graph, ImportReport, RuleViolation};
pub use id::{AgentId, ContributionId, GroundId, Signature};
pub use identity::Identity;
pub use store::{GroundInfo, Store, StoreError};
pub use validation::{Assessment, Conflict, Evaluation, Policy, Status};
