//! Headless `RemoteRadio` client.
//!
//! ```no_run
//! use rr_client::{RadioClient, Scope};
//!
//! # async fn demo() -> Result<(), rr_client::ClientError> {
//! let (client, audio) = RadioClient::builder()
//!     .server("newyork.remote.radio", 54736)
//!     .username("W1AW")
//!     .scope(Scope::Channel("NYC Stereo".into()))
//!     .build()?;
//! client.start();
//! // Hand `audio.input` to your capture callback and `audio.output` to your
//! // playback callback; everything else is reached through `client`.
//! # drop(audio);
//! # Ok(())
//! # }
//! ```
//!
//! A [`Scope`] confines the client to one channel and its sub-channels, so an
//! application can expose a single room instead of the whole server.

mod client;
mod config;
mod event;
mod handler;
mod io;
mod shared;

pub use client::{ClientError, RadioClient};
pub use config::{ClientBuilder, ClientSettings, Scope};
pub use event::{ChannelInfo, Event, UserInfo};
pub use io::{AudioEndpoints, InputPipe};
