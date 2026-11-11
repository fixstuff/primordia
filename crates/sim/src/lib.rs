//! LifeAPI simulation core.
//! Pure: no I/O, no clock, no threads. `World::tick` is the only way time moves.

pub mod brain;
pub mod det;
pub mod genome;
pub mod grid;
pub mod rng;
pub mod scenario;
pub mod world;

pub use world::World;
