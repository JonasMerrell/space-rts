//! Shared simulation for the space RTS: orbital mechanics, game rules and the transfer planner.
//! Runs identically on the server (authoritative) and the client (prediction / planning UI).
pub mod assist;
pub mod bastion;
pub mod asteroid;
pub mod bot;
pub mod cloak;
pub mod combat;
pub mod fog;
pub mod fleet;
pub mod game;
pub mod intercept;
pub mod kepler;
pub mod ordnance;
pub mod plan;
pub mod reroute;
pub mod shiplog;
pub mod station;
pub mod tech;
pub mod traj;
pub mod transit;
pub mod system;
pub mod v2;
pub mod waitlist;

pub use system::{Body, BodyId, BodyKind, System, SystemSize};
pub use v2::{v2, V2};
