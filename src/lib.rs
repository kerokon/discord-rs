//! Client library for the [Discord](https://discord.com) API.
//!
//! The Discord API can be divided into three main components: the RESTful API
//! to which calls can be made to take actions, a websocket-based permanent
//! connection over which state updates are received, and the voice calling
//! system.
//!
//! Log in to Discord with `Discord::new`, `new_cache`, or `from_bot_token` as appropriate.
//! The resulting value can be used to make REST API calls to post messages and manipulate Discord
//! state. Calling `connect()` will open a websocket connection, through which events can be
//! received. These two channels are enough to write a simple chatbot which can
//! read and respond to messages.
//!
//! For more in-depth tracking of Discord state, a `State` can be seeded with
//! the `ReadyEvent` obtained when opening a `Connection` and kept updated with
//! the events received over it.
//!
#![cfg_attr(
	not(feature = "voice"),
	doc = "*<b>NOTE</b>: The library has been compiled without voice support.*"
)]
//! To join voice servers, call `Connection::voice` to get a `VoiceConnection` and use `connect`
//! to join a channel, then `play` and `stop` to control playback. Manipulating deaf/mute state
//! and receiving audio are also possible.
//!
//! For examples, see the `examples` directory in the source tree.
#![warn(missing_docs)]
#![allow(deprecated)]

extern crate base64;
extern crate chrono;
extern crate flate2;
extern crate hyper;
extern crate hyper_native_tls;
extern crate multipart;
extern crate serde;
extern crate websocket;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate bitflags;
#[macro_use]
extern crate log;
#[cfg(feature = "voice")]
extern crate byteorder;
#[cfg(feature = "voice")]
extern crate opus;
#[cfg(feature = "voice")]
extern crate sodiumoxide;

use std::collections::BTreeMap;
use std::time;
use serde_json::to_string;

type Object = serde_json::Map<String, serde_json::Value>;

mod connection;
mod error;
mod ratelimit;
mod state;
#[cfg(feature = "voice")]
pub mod voice;

macro_rules! cdn_concat {
	($e:expr) => {
		// Out of everything, only the CDN still uses the old domain.
		concat!("https://cdn.discordapp.com", $e)
	};
}

#[macro_use]
mod serial;
pub mod builders;
pub mod model;
pub mod interact;

use builders::*;
pub use connection::Connection;
pub use error::{Error, Result};
use interact::Interact;
use model::*;
use ratelimit::RateLimits;
pub use state::{ChannelRef, State};

const USER_AGENT: &'static str = concat!(
	"DiscordBot (https://github.com/SpaceManiac/discord-rs, ",
	env!("CARGO_PKG_VERSION"),
	")"
);
macro_rules! api_concat {
	($e:expr) => {
		concat!("https://discord.com/api/v6", $e)
	};
}
macro_rules! status_concat {
	($e:expr) => {
		concat!("https://status.discord.com/api/v2", $e)
	};
}

macro_rules! request {
	($self_:ident, $method:ident($body:expr), $url:expr, $($rest:tt)*) => {{
		let path = format!(api_concat!($url), $($rest)*);
		$self_.request(&path, || $self_.client.$method(&path).body(&$body))?
	}};
	($self_:ident, $method:ident, $url:expr, $($rest:tt)*) => {{
		let path = format!(api_concat!($url), $($rest)*);
		$self_.request(&path, || $self_.client.$method(&path))?
	}};
	($self_:ident, $method:ident($body:expr), $url:expr) => {{
		let path = api_concat!($url);
		$self_.request(path, || $self_.client.$method(path).body(&$body))?
	}};
	($self_:ident, $method:ident, $url:expr) => {{
		let path = api_concat!($url);
		$self_.request(path, || $self_.client.$method(path))?
	}};
}

/// Client for the Discord REST API.
///
/// Log in to the API with a user's email and password using `new()`. Call
/// `connect()` to create a `Connection` on which to receive events. If desired,
/// use `logout()` to invalidate the token when done. Other methods manipulate
/// the Discord REST API.
pub struct Discord {
	rate_limits: RateLimits,
	client: hyper::Client,
	token: String,
}

fn tls_client() -> hyper::Client {
	let tls = hyper_native_tls::NativeTlsClient::new().expect("Error initializing NativeTlsClient");
	let connector = hyper::net::HttpsConnector::new(tls);
	hyper::Client::with_connector(connector)
}

impl Discord {
	/// Log in to the Discord Rest API and acquire a token.
	#[deprecated(note = "Login automation is not recommended. Use `from_user_token` instead.")]
	pub fn new(email: &str, password: &str) -> Result<Discord> {
		let mut map = BTreeMap::new();
		map.insert("email", email);
		map.insert("password", password);

		let client = tls_client();
		let response = check_status(
			client
				.post(api_concat!("/auth/login"))
				.header(hyper::header::ContentType::json())
				.header(hyper::header::UserAgent(USER_AGENT.to_owned()))
				.body(&serde_json::to_string(&map)?)
				.send(),
		)?;
		let mut json: BTreeMap<String, String> = serde_json::from_reader(response)?;
		let token = match json.remove("token") {
			Some(token) => token,
			None => {
				return Err(Error::Protocol(
					"Response missing \"token\" in Discord::new()",
				))
			}
		};
		Ok(Discord {
			rate_limits: RateLimits::default(),
			client: client,
			token: token,
		})
	}

	/// Log in to the Discord Rest API, possibly using a cached login token.
	///
	/// Cached login tokens are keyed to the email address and will be read from
	/// and written to the specified path. If no cached token was found and no
	/// password was specified, an error is returned.
	#[deprecated(note = "Login automation is not recommended. Use `from_user_token` instead.")]
	#[allow(deprecated)]
	pub fn new_cache<P: AsRef<std::path::Path>>(
		path: P,
		email: &str,
		password: Option<&str>,
	) -> Result<Discord> {
		use std::fs::File;
		use std::io::{BufRead, BufReader, Write};

		// Read the cache, looking for our token
		let path = path.as_ref();
		let mut initial_token: Option<String> = None;
		if let Ok(file) = File::open(path) {
			for line in BufReader::new(file).lines() {
				let line = line?;
				let parts: Vec<_> = line.split('\t').collect();
				if parts.len() == 2 && parts[0] == email {
					initial_token = Some(parts[1].trim().into());
					break;
				}
			}
		}

		// Perform the login
		let discord = if let Some(ref initial_token) = initial_token {
			let mut map = BTreeMap::new();
			map.insert("email", email);
			if let Some(password) = password {
				map.insert("password", password);
			}

			let client = tls_client();
			let response = check_status(
				client
					.post(api_concat!("/auth/login"))
					.header(hyper::header::ContentType::json())
					.header(hyper::header::UserAgent(USER_AGENT.to_owned()))
					.header(hyper::header::Authorization(initial_token.clone()))
					.body(&serde_json::to_string(&map)?)
					.send(),
			)?;
			let mut json: BTreeMap<String, String> = serde_json::from_reader(response)?;
			let token = match json.remove("token") {
				Some(token) => token,
				None => {
					return Err(Error::Protocol(
						"Response missing \"token\" in Discord::new()",
					))
				}
			};
			Discord {
				rate_limits: RateLimits::default(),
				client: client,
				token: token,
			}
		} else if let Some(password) = password {
			Discord::new(email, password)?
		} else {
			return Err(Error::Other(
				"No password was specified and no cached token was found",
			));
		};

		// Write the token back out, if needed
		if initial_token.as_ref() != Some(&discord.token) {
			let mut tokens = Vec::new();
			tokens.push(format!("{}\t{}", email, discord.token));
			if let Ok(file) = File::open(path) {
				for line in BufReader::new(file).lines() {
					let line = line?;
					if line.split('\t').next() != Some(email) {
						tokens.push(line);
					}
				}
			}
			let mut file = File::create(path)?;
			for line in tokens {
				file.write_all(line.as_bytes())?;
				file.write_all(&[b'\n'])?;
			}
		}

		Ok(discord)
	}

	fn from_token_raw(token: String) -> Discord {
		Discord {
			rate_limits: RateLimits::default(),
			client: tls_client(),
			token: token,
		}
	}

	/// Log in as a bot account using the given authentication token.
	///
	/// The token will automatically be prefixed with "Bot ".
	pub fn from_bot_token(token: &str) -> Result<Discord> {
		Ok(Discord::from_token_raw(format!("Bot {}", token.trim())))
	}

	/// Log in as a user account using the given authentication token.
	pub fn from_user_token(token: &str) -> Result<Discord> {
		Ok(Discord::from_token_raw(token.trim().to_owned()))
	}

	/// Log out from the Discord API, invalidating this clients's token.
	#[deprecated(note = "Accomplishes nothing and may fail for no reason.")]
	pub fn logout(self) -> Result<()> {
		let map = json! {{
			"provider": null,
			"token": null,
		}};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(self, post(body), "/auth/logout"))
	}

	fn request<'a, F: Fn() -> hyper::client::RequestBuilder<'a>>(
		&self,
		url: &str,
		f: F,
	) -> Result<hyper::client::Response> {
		self.rate_limits.pre_check(url);
		let f2 = || {
			f().header(hyper::header::ContentType::json())
				.header(hyper::header::Authorization(self.token.clone()))
		};
		let result = retry(&f2);
		if let Ok(response) = result.as_ref() {
			if self.rate_limits.post_update(url, response) {
				// we were rate limited, we have slept, it is time to retry
				// the request once. if it fails the second time, give up
				debug!("Retrying after having been ratelimited");
				let result = retry(f2);
				if let Ok(response) = result.as_ref() {
					self.rate_limits.post_update(url, response);
				}
				return check_status(result);
			}
		}
		check_status(result)
	}

	/// Create a channel.
	pub fn create_channel(
		&self,
		server: ServerId,
		name: &str,
		kind: ChannelType,
	) -> Result<Channel> {
		let map = json! {{
			"name": name,
			"type": kind.num(),
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds/{}/channels", server);
		Channel::decode(serde_json::from_reader(response)?)
	}

	/// Get the list of channels in a server.
	pub fn get_server_channels(&self, server: ServerId) -> Result<Vec<PublicChannel>> {
		let response = request!(self, get, "/guilds/{}/channels", server);
		decode_array(serde_json::from_reader(response)?, PublicChannel::decode)
	}

	/// Get information about a channel.
	pub fn get_channel(&self, channel: ChannelId) -> Result<Channel> {
		let response = request!(self, get, "/channels/{}", channel);
		Channel::decode(serde_json::from_reader(response)?)
	}

	/// Edit a channel's details. See `EditChannel` for the editable fields.
	///
	/// ```ignore
	/// // Edit a channel's name and topic
	/// discord.edit_channel(channel_id, "general", |ch| ch
	///     .topic("Welcome to the general chat!")
	/// );
	/// ```
	pub fn edit_channel<F: FnOnce(EditChannel) -> EditChannel>(
		&self,
		channel: ChannelId,
		f: F,
	) -> Result<PublicChannel> {
		// Work around the fact that this supposed PATCH call actually requires all fields
		let mut map = Object::new();
		match self.get_channel(channel)? {
			Channel::Private(_) => return Err(Error::Other("Can not edit private channels")),
			Channel::Public(channel) => {
				map.insert("name".into(), channel.name.into());
				map.insert("position".into(), channel.position.into());

				match channel.kind {
					ChannelType::Text => {
						map.insert("topic".into(), json!(channel.topic));
					}
					ChannelType::Voice => {
						map.insert("bitrate".into(), json!(channel.bitrate));
						map.insert("user_limit".into(), json!(channel.user_limit));
					}
					_ => {
						return Err(Error::Other(stringify!(format!(
							"Unreachable channel type: {:?}",
							channel.kind
						))))
					}
				}
			}
			Channel::Group(group) => {
				map.insert("name".into(), json!(group.name));
			}
			Channel::Category(_) => {}
			Channel::News => {}
			Channel::Store => {}
		};
		let map = EditChannel::__apply(f, map);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/channels/{}", channel);
		PublicChannel::decode(serde_json::from_reader(response)?)
	}

	/// Delete a channel.
	pub fn delete_channel(&self, channel: ChannelId) -> Result<Channel> {
		let response = request!(self, delete, "/channels/{}", channel);
		Channel::decode(serde_json::from_reader(response)?)
	}

	/// Indicate typing on a channel for the next 5 seconds.
	pub fn broadcast_typing(&self, channel: ChannelId) -> Result<()> {
		check_empty(request!(self, post, "/channels/{}/typing", channel))
	}

	/// Get a single message by ID from a given channel.
	pub fn get_message(&self, channel: ChannelId, message: MessageId) -> Result<Message> {
		let response = request!(self, get, "/channels/{}/messages/{}", channel, message);
		from_reader(response)
	}

	/// Get messages in the backlog for a given channel.
	///
	/// The `what` argument should be one of the options in the `GetMessages`
	/// enum, and will determine which messages will be returned. A message
	/// limit can also be specified, and defaults to 50. More recent messages
	/// will appear first in the list.
	pub fn get_messages(
		&self,
		channel: ChannelId,
		what: GetMessages,
		limit: Option<u64>,
	) -> Result<Vec<Message>> {
		use std::fmt::Write;
		let mut url = format!(
			api_concat!("/channels/{}/messages?limit={}"),
			channel,
			limit.unwrap_or(50)
		);
		match what {
			GetMessages::MostRecent => {}
			GetMessages::Before(id) => {
				let _ = write!(url, "&before={}", id);
			}
			GetMessages::After(id) => {
				let _ = write!(url, "&after={}", id);
			}
			GetMessages::Around(id) => {
				let _ = write!(url, "&around={}", id);
			}
		}
		let response = self.request(&url, || self.client.get(&url))?;
		from_reader(response)
	}

	/// Gets the pinned messages for a given channel.
	pub fn get_pinned_messages(&self, channel: ChannelId) -> Result<Vec<Message>> {
		let response = request!(self, get, "/channels/{}/pins", channel);
		from_reader(response)
	}

	/// Pin the given message to the given channel.
	///
	/// Requires that the logged in account have the "MANAGE_MESSAGES" permission.
	pub fn pin_message(&self, channel: ChannelId, message: MessageId) -> Result<()> {
		check_empty(request!(
			self,
			put,
			"/channels/{}/pins/{}",
			channel,
			message
		))
	}

	/// Removes the given message from being pinned to the given channel.
	///
	/// Requires that the logged in account have the "MANAGE_MESSAGES" permission.
	pub fn unpin_message(&self, channel: ChannelId, message: MessageId) -> Result<()> {
		check_empty(request!(
			self,
			delete,
			"/channels/{}/pins/{}",
			channel,
			message
		))
	}

	/// Send a message to a given channel.
	///
	/// The `nonce` will be returned in the result and also transmitted to other
	/// clients. The empty string is a good default if you don't care.
	pub fn send_message_ex<F: FnOnce(SendMessage) -> SendMessage>(
		&self,
		channel: ChannelId,
		f: F,
	) -> Result<Message> {
		let map = SendMessage::__build(f);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/channels/{}/messages", channel);
		from_reader(response)
	}



	/// Edit a previously posted message.
	///
	/// Requires that either the message was posted by this user, or this user
	/// has permission to manage other members' messages.
	///
	/// Not all fields can be edited; see the [docs] for more.
	/// [docs]: https://discord.com/developers/docs/resources/channel#edit-message
	pub fn edit_message_ex<F: FnOnce(SendMessage) -> SendMessage>(
		&self,
		channel: ChannelId,
		message: MessageId,
		f: F,
	) -> Result<Message> {
		let map = SendMessage::__build(f);
		let body = serde_json::to_string(&map)?;
		let response = request!(
			self,
			patch(body),
			"/channels/{}/messages/{}",
			channel,
			message
		);
		from_reader(response)
	}

	/// Send a message to a given channel.
	///
	/// The `nonce` will be returned in the result and also transmitted to other
	/// clients. The empty string is a good default if you don't care.
	pub fn send_message(
		&self,
		channel: ChannelId,
		text: &str,
		nonce: &str,
		tts: bool,
	) -> Result<Message> {
		self.send_message_ex(channel, |b| b.content(text).nonce(nonce).tts(tts))
	}

	/// Edit a previously posted message.
	///
	/// Requires that either the message was posted by this user, or this user
	/// has permission to manage other members' messages.
	pub fn edit_message(
		&self,
		channel: ChannelId,
		message: MessageId,
		text: &str,
	) -> Result<Message> {
		self.edit_message_ex(channel, message, |b| b.content(text))
	}

	/// Delete a previously posted message.
	///
	/// Requires that either the message was posted by this user, or this user
	/// has permission to manage other members' messages.
	pub fn delete_message(&self, channel: ChannelId, message: MessageId) -> Result<()> {
		check_empty(request!(
			self,
			delete,
			"/channels/{}/messages/{}",
			channel,
			message
		))
	}

	/// Bulk deletes a list of `MessageId`s from a given channel.
	///
	/// A minimum of 2 unique messages and a maximum of 100 unique messages may
	/// be supplied, otherwise an `Error::Other` will be returned.
	///
	/// Each MessageId *should* be unique as duplicates will be removed from the
	/// array before being sent to the Discord API.
	///
	/// Only bots can use this endpoint. Regular user accounts can not use this
	/// endpoint under any circumstance.
	///
	/// Requires that either the message was posted by this user, or this user
	/// has permission to manage other members' messages.
	pub fn delete_messages(&self, channel: ChannelId, messages: &[MessageId]) -> Result<()> {
		// Create a Vec of the underlying u64's of the message ids, then remove
		// duplicates in it.
		let mut ids: Vec<u64> = messages.into_iter().map(|m| m.0).collect();
		ids.sort();
		ids.dedup();

		if ids.len() < 2 {
			return Err(Error::Other("A minimum of 2 message ids must be supplied"));
		} else if ids.len() > 100 {
			return Err(Error::Other("A maximum of 100 message ids may be supplied"));
		}

		let map = json! {{ "messages": ids }};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(
			self,
			post(body),
			"/channels/{}/messages/bulk_delete",
			channel
		))
	}

	/// Send some embedded rich content attached to a message on a given channel.
	///
	/// See the `EmbedBuilder` struct for the editable fields.
	/// `text` may be empty.
	pub fn send_embed<F: FnOnce(EmbedBuilder) -> EmbedBuilder>(
		&self,
		channel: ChannelId,
		text: &str,
		f: F,
	) -> Result<Message> {
		self.send_message_ex(channel, |b| b.content(text).embed(f))
	}

	/// Edit the embed portion of a previously posted message.
	///
	/// The text is unmodified, but the previous embed is entirely replaced.
	pub fn edit_embed<F: FnOnce(EmbedBuilder) -> EmbedBuilder>(
		&self,
		channel: ChannelId,
		message: MessageId,
		f: F,
	) -> Result<Message> {
		self.edit_message_ex(channel, message, |b| b.embed(f))
	}

	/// Send a file attached to a message on a given channel.
	///
	/// The `text` is allowed to be empty, but the filename must always be specified.
	pub fn send_file<R: ::std::io::Read>(
		&self,
		channel: ChannelId,
		text: &str,
		mut file: R,
		filename: &str,
	) -> Result<Message> {
		use std::io::Write;

		let url = match hyper::Url::parse(&format!(api_concat!("/channels/{}/messages"), channel)) {
			Ok(url) => url,
			Err(_) => return Err(Error::Other("Invalid URL in send_file")),
		};
		// NB: We're NOT using the Hyper itegration of multipart in order not to wrestle with the openssl-sys dependency hell.
		let cr = multipart::mock::ClientRequest::default();
		let mut multi = multipart::client::Multipart::from_request(cr)?;
		multi.write_text("content", text)?;
		multi.write_stream("file", &mut file, Some(filename), None)?;
		let http_buffer: multipart::mock::HttpBuffer = multi.send()?;
		fn multipart_mime(bound: &str) -> hyper::mime::Mime {
			use hyper::mime::{Attr, Mime, SubLevel, TopLevel, Value};
			Mime(
				TopLevel::Multipart,
				SubLevel::Ext("form-data".into()),
				vec![(Attr::Ext("boundary".into()), Value::Ext(bound.into()))],
			)
		}

		let tls = hyper_native_tls::NativeTlsClient::new().expect("Error initializing NativeTlsClient");
		let connector = hyper::net::HttpsConnector::new(tls);
		let mut request = hyper::client::Request::with_connector(hyper::method::Method::Post, url, &connector)?;
		request
			.headers_mut()
			.set(hyper::header::Authorization(self.token.clone()));
		request
			.headers_mut()
			.set(hyper::header::UserAgent(USER_AGENT.to_owned()));
		request
			.headers_mut()
			.set(hyper::header::ContentType(multipart_mime(
				&http_buffer.boundary,
			)));
		let mut request = request.start()?;
		request.write(&http_buffer.buf[..])?;
		Message::decode(serde_json::from_reader(check_status(request.send())?)?)
	}

	pub fn interact(
		&self,
		interact:Interact
	) -> Result<()> {
		use std::io::Write;

		let url = match hyper::Url::parse("https://discord.com/api/v9/interactions") {
			Ok(url) => url,
			Err(_) => return Err(Error::Other("Invalid URL in interact")),
		};
		// NB: We're NOT using the Hyper itegration of multipart in order not to wrestle with the openssl-sys dependency hell.
		let cr = multipart::mock::ClientRequest::default();
		let mut multi = multipart::client::Multipart::from_request(cr)?;
		multi.write_text("payload_json", to_string(&interact)?)?;
		let http_buffer: multipart::mock::HttpBuffer = multi.send()?;
		fn multipart_mime(bound: &str) -> hyper::mime::Mime {
			use hyper::mime::{Attr, Mime, SubLevel, TopLevel, Value};
			Mime(
				TopLevel::Multipart,
				SubLevel::Ext("form-data".into()),
				vec![(Attr::Ext("boundary".into()), Value::Ext(bound.into()))],
			)
		}

		let tls = hyper_native_tls::NativeTlsClient::new().expect("Error initializing NativeTlsClient");
		let connector = hyper::net::HttpsConnector::new(tls);
		let mut request = hyper::client::Request::with_connector(hyper::method::Method::Post, url, &connector)?;
		request
			.headers_mut()
			.set(hyper::header::Authorization(self.token.clone()));
		request
			.headers_mut()
			.set(hyper::header::UserAgent(USER_AGENT.to_owned()));
		request
			.headers_mut()
			.set(hyper::header::ContentType(multipart_mime(
				&http_buffer.boundary,
			)));
		let mut request = request.start()?;
		request.write(&http_buffer.buf[..])?;
		check_status(request.send())?;
		Ok(())
	}

	/// Acknowledge this message as "read" by this client.
	pub fn ack_message(&self, channel: ChannelId, message: MessageId) -> Result<()> {
		check_empty(request!(
			self,
			post,
			"/channels/{}/messages/{}/ack",
			channel,
			message
		))
	}

	/// Create permissions for a `Channel` for a `Member` or `Role`.
	///
	/// # Examples
	///
	/// An example of creating channel role permissions for a `Member`:
	///
	/// ```ignore
	/// use discord::model::{PermissionOverwriteType, permissions};
	///
	/// // Assuming that a `Discord` instance, member, and channel have already
	/// // been defined previously.
	/// let target = PermissionOverwrite {
	///     kind: PermissionOverwriteType::Member(member.user.id),
	///     allow: permissions::VOICE_CONNECT | permissions::VOICE_SPEAK,
	///     deny: permissions::VOICE_MUTE_MEMBERS | permissions::VOICE_MOVE_MEMBERS,
	/// };
	/// let result = discord.create_permission(channel.id, target);
	/// ```
	///
	/// The same can similarly be accomplished for a `Role`:
	///
	/// ```ignore
	/// use discord::model::{PermissionOverwriteType, permissions};
	///
	/// // Assuming that a `Discord` instance, role, and channel have already
	/// // been defined previously.
	/// let target = PermissionOverwrite {
	///	    kind: PermissionOverwriteType::Role(role.id),
	///	    allow: permissions::VOICE_CONNECT | permissions::VOICE_SPEAK,
	///	    deny: permissions::VOICE_MUTE_MEMBERS | permissions::VOICE_MOVE_MEMBERS,
	///	};
	/// let result = discord.create_permission(channel.id, target);
	/// ```
	pub fn create_permission(&self, channel: ChannelId, target: PermissionOverwrite) -> Result<()> {
		let (id, kind) = match target.kind {
			PermissionOverwriteType::Member(id) => (id.0, "member"),
			PermissionOverwriteType::Role(id) => (id.0, "role"),
		};
		let map = json! {{
			"id": id,
			"kind": kind,
			"allow": target.allow.bits(),
			"deny": target.deny.bits(),
		}};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(
			self,
			put(body),
			"/channels/{}/permissions/{}",
			channel,
			id
		))
	}

	/// Delete a `Member` or `Role`'s permissions for a `Channel`.
	///
	/// # Examples
	///
	/// Delete a `Member`'s permissions for a `Channel`:
	///
	/// ```ignore
	/// use discord::model::PermissionOverwriteType;
	///
	/// // Assuming that a `Discord` instance, channel, and member have already
	/// // been previously defined.
	/// let target = PermissionOverwriteType::Member(member.user.id);
	/// let response = discord.delete_permission(channel.id, target);
	/// ```
	///
	/// The same can be accomplished for a `Role` similarly:
	///
	/// ```ignore
	/// use discord::model::PermissionOverwriteType;
	///
	/// // Assuming that a `Discord` instance, channel, and role have already
	/// // been previously defined.
	/// let target = PermissionOverwriteType::Role(role.id);
	/// let response = discord.delete_permission(channel.id, target);
	/// ```
	pub fn delete_permission(
		&self,
		channel: ChannelId,
		permission_type: PermissionOverwriteType,
	) -> Result<()> {
		let id = match permission_type {
			PermissionOverwriteType::Member(id) => id.0,
			PermissionOverwriteType::Role(id) => id.0,
		};
		check_empty(request!(
			self,
			delete,
			"/channels/{}/permissions/{}",
			channel,
			id
		))
	}

	/// Add a `Reaction` to a `Message`.
	///
	/// # Examples
	/// Add an unicode emoji to a `Message`:
	///
	/// ```ignore
	/// // Assuming that a `Discord` instance, channel, message have
	/// // already been previously defined.
	/// use discord::model::ReactionEmoji;
	///
	/// let _ = discord.add_reaction(&channel.id, message.id, ReactionEmoji::Unicode("👌".to_string));
	/// ```
	///
	/// Add a custom emoji to a `Message`:
	///
	/// ```ignore
	/// // Assuming that a `Discord` instance, channel, message have
	/// // already been previously defined.
	/// use discord::model::{EmojiId, ReactionEmoji};
	///
	/// let _ = discord.add_reaction(&channel.id, message.id, ReactionEmoji::Custom {
	///     name: "ThisIsFine",
	///     id: EmojiId(1234)
	/// });
	/// ```
	///
	/// Requires the `ADD_REACTIONS` permission.
	pub fn add_reaction(
		&self,
		channel: ChannelId,
		message: MessageId,
		emoji: ReactionEmoji,
	) -> Result<()> {
		let emoji = match emoji {
			ReactionEmoji::Custom { name, id } => format!("{}:{}", name, id.0),
			ReactionEmoji::Unicode(name) => name,
		};
		check_empty(request!(
			self,
			put,
			"/channels/{}/messages/{}/reactions/{}/@me",
			channel,
			message,
			emoji
		))
	}

	/// Delete a `Reaction` from a `Message`.
	///
	/// # Examples
	/// Delete a `Reaction` from a `Message` (unicode emoji):
	///
	/// ```ignore
	/// // Assuming that a `Discord` instance, channel, message, state have
	/// // already been previously defined.
	/// use discord::model::ReactionEmoji;
	///
	/// let _ = discord.delete_reaction(&channel.id, message.id, None, ReactionEmoji::Unicode("👌".to_string()));
	/// ```
	///
	/// Delete your `Reaction` from a `Message` (custom emoji):
	///
	/// ```ignore
	/// // Assuming that a `Discord` instance, channel, message have
	/// // already been previously defined.
	/// use discord::model::ReactionEmoji;
	///
	/// let _ = discord.delete_reaction(&channel.id, message.id, None, ReactionEmoji::Custom {
	///	    name: "ThisIsFine",
	///     id: EmojiId(1234)
	/// });
	/// ```
	///
	/// Delete someone else's `Reaction` from a `Message` (custom emoji):
	///
	/// ```ignore
	/// // Assuming that a `Discord` instance, channel, message have
	/// // already been previously defined.
	/// use discord::model::{EmojiId, ReactionEmoji};
	///
	/// let _ = discord.delete_reaction(&channel.id, message.id, Some(UserId(1234)), ReactionEmoji::Custom {
	///     name: "ThisIsFine",
	///     id: EmojiId(1234)
	/// });
	/// ```
	///
	/// Requires `MANAGE_MESSAGES` if deleting someone else's `Reaction`.
	pub fn delete_reaction(
		&self,
		channel: ChannelId,
		message: MessageId,
		user_id: Option<UserId>,
		emoji: ReactionEmoji,
	) -> Result<()> {
		let emoji = match emoji {
			ReactionEmoji::Custom { name, id } => format!("{}:{}", name, id.0),
			ReactionEmoji::Unicode(name) => name,
		};
		let endpoint = format!(
			"/channels/{}/messages/{}/reactions/{}/{}",
			channel,
			message,
			emoji,
			match user_id {
				Some(id) => id.0.to_string(),
				None => "@me".to_string(),
			}
		);
		check_empty(request!(self, delete, "{}", endpoint))
	}

	/// Get reactors for the `Emoji` in a `Message`.
	///
	/// The default `limit` is 50. The optional value of `after` is the ID of
	/// the user to retrieve the next reactions after.
	pub fn get_reactions(
		&self,
		channel: ChannelId,
		message: MessageId,
		emoji: ReactionEmoji,
		limit: Option<i32>,
		after: Option<UserId>,
	) -> Result<Vec<User>> {
		let emoji = match emoji {
			ReactionEmoji::Custom { name, id } => format!("{}:{}", name, id.0),
			ReactionEmoji::Unicode(name) => name,
		};
		let mut endpoint = format!(
			"/channels/{}/messages/{}/reactions/{}?limit={}",
			channel,
			message,
			emoji,
			limit.unwrap_or(50)
		);

		if let Some(amount) = after {
			use std::fmt::Write;
			let _ = write!(endpoint, "&after={}", amount);
		}

		let response = request!(self, get, "{}", endpoint);
		from_reader(response)
	}

	/// Get the list of servers this user knows about.
	pub fn get_servers(&self) -> Result<Vec<ServerInfo>> {
		let response = request!(self, get, "/users/@me/guilds");
		from_reader(response)
	}

	/// Gets a specific server.
	pub fn get_server(&self, server_id: ServerId) -> Result<Server> {
		let response = request!(self, get, "/guilds/{}", server_id);
		from_reader(response)
	}

	/// Gets the list of a specific server's members.
	pub fn get_server_members(&self, server_id: ServerId) -> Result<Vec<Member>> {
		let response = request!(self, get, "/guilds/{}/members", server_id);
		from_reader(response)
	}

	/// Create a new server with the given name.
	pub fn create_server(&self, name: &str, region: &str, icon: Option<&str>) -> Result<Server> {
		let map = json! {{
			"name": name,
			"region": region,
			"icon": icon,
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds");
		from_reader(response)
	}

	/// Edit a server's information. See `EditServer` for the editable fields.
	///
	/// ```ignore
	/// // Rename a server
	/// discord.edit_server(server_id, |server| server.name("My Cool Server"));
	/// // Edit many properties at once
	/// discord.edit_server(server_id, |server| server
	///     .name("My Cool Server")
	///     .icon(Some("data:image/jpg;base64,..."))
	///     .afk_timeout(300)
	///     .region("us-south")
	/// );
	/// ```
	pub fn edit_server<F: FnOnce(EditServer) -> EditServer>(
		&self,
		server_id: ServerId,
		f: F,
	) -> Result<Server> {
		let map = EditServer::__build(f);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/guilds/{}", server_id);
		from_reader(response)
	}

	/// Leave the given server.
	pub fn leave_server(&self, server: ServerId) -> Result<Server> {
		let response = request!(self, delete, "/users/@me/guilds/{}", server);
		from_reader(response)
	}

	/// Delete the given server. Only available to the server owner.
	pub fn delete_server(&self, server: ServerId) -> Result<Server> {
		let response = request!(self, delete, "/guilds/{}", server);
		from_reader(response)
	}

	/// Creates an emoji in a server.
	///
	/// `read_image` may be used to build an `image` string. Requires that the
	/// logged in account be a user and have the `ADMINISTRATOR` or
	/// `MANAGE_EMOJIS` permission.
	pub fn create_emoji(&self, server: ServerId, name: &str, image: &str) -> Result<Emoji> {
		let map = json! {{
			"name": name,
			"image": image,
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds/{}/emojis", server);
		from_reader(response)
	}

	/// Edits a server's emoji.
	///
	/// Requires that the logged in account be a user and have the
	/// `ADMINISTRATOR` or `MANAGE_EMOJIS` permission.
	pub fn edit_emoji(&self, server: ServerId, emoji: EmojiId, name: &str) -> Result<Emoji> {
		let map = json! {{
			"name": name
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/guilds/{}/emojis/{}", server, emoji);
		from_reader(response)
	}

	/// Delete an emoji in a server.
	///
	/// Requires that the logged in account be a user and have the
	/// `ADMINISTRATOR` or `MANAGE_EMOJIS` permission.
	pub fn delete_emoji(&self, server: ServerId, emoji: EmojiId) -> Result<()> {
		check_empty(request!(
			self,
			delete,
			"/guilds/{}/emojis/{}",
			server,
			emoji
		))
	}

	/// Get the ban list for the given server.
	pub fn get_bans(&self, server: ServerId) -> Result<Vec<Ban>> {
		let response = request!(self, get, "/guilds/{}/bans", server);
		from_reader(response)
	}

	/// Ban a user from the server, optionally deleting their recent messages.
	///
	/// Zero may be passed for `delete_message_days` if no deletion is desired.
	pub fn add_ban(&self, server: ServerId, user: UserId, delete_message_days: u32) -> Result<()> {
		check_empty(request!(
			self,
			put,
			"/guilds/{}/bans/{}?delete_message_days={}",
			server,
			user,
			delete_message_days
		))
	}

	/// Unban a user from the server.
	pub fn remove_ban(&self, server: ServerId, user: UserId) -> Result<()> {
		check_empty(request!(self, delete, "/guilds/{}/bans/{}", server, user))
	}

	/// Extract information from an invite.
	///
	/// The invite should either be a URL of the form `http://discord.gg/CODE`,
	/// or a string containing just the `CODE`.
	pub fn get_invite(&self, invite: &str) -> Result<Invite> {
		let invite = resolve_invite(invite);
		let response = request!(self, get, "/invite/{}", invite);
		Invite::decode(serde_json::from_reader(response)?)
	}

	/// Get the active invites for a server.
	pub fn get_server_invites(&self, server: ServerId) -> Result<Vec<RichInvite>> {
		let response = request!(self, get, "/guilds/{}/invites", server);
		decode_array(serde_json::from_reader(response)?, RichInvite::decode)
	}

	/// Get the active invites for a channel.
	pub fn get_channel_invites(&self, channel: ChannelId) -> Result<Vec<RichInvite>> {
		let response = request!(self, get, "/channels/{}/invites", channel);
		decode_array(serde_json::from_reader(response)?, RichInvite::decode)
	}

	/// Accept an invite. See `get_invite` for details.
	pub fn accept_invite(&self, invite: &str) -> Result<Invite> {
		let invite = resolve_invite(invite);
		let response = request!(self, post, "/invite/{}", invite);
		Invite::decode(serde_json::from_reader(response)?)
	}

	/// Create an invite to a channel.
	///
	/// Passing 0 for `max_age` or `max_uses` means no limit. `max_age` should
	/// be specified in seconds.
	pub fn create_invite(
		&self,
		channel: ChannelId,
		max_age: u64,
		max_uses: u64,
		temporary: bool,
	) -> Result<RichInvite> {
		let map = json! {{
			"validate": null,
			"max_age": max_age,
			"max_uses": max_uses,
			"temporary": temporary,
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/channels/{}/invites", channel);
		RichInvite::decode(serde_json::from_reader(response)?)
	}

	/// Delete an invite. See `get_invite` for details.
	pub fn delete_invite(&self, invite: &str) -> Result<Invite> {
		let invite = resolve_invite(invite);
		let response = request!(self, delete, "/invite/{}", invite);
		Invite::decode(serde_json::from_reader(response)?)
	}

	/// Retrieve a member object for a server given the member's user id.
	pub fn get_member(&self, server: ServerId, user: UserId) -> Result<Member> {
		let response = request!(self, get, "/guilds/{}/members/{}", server, user);
		from_reader(response)
	}

	/// Edit the list of roles assigned to a member of a server.
	pub fn edit_member_roles(
		&self,
		server: ServerId,
		user: UserId,
		roles: &[RoleId],
	) -> Result<()> {
		self.edit_member(server, user, |m| m.roles(roles))
	}

	/// Add a role to a member of a server.
	pub fn add_member_role(&self, server: ServerId, user: UserId, role: RoleId) -> Result<()> {
		check_empty(request!(
			self,
			put,
			"/guilds/{}/members/{}/roles/{}",
			server,
			user,
			role
		))
	}

	/// Remove a role for a member of a server.
	pub fn remove_member_role(&self, server: ServerId, user: UserId, role: RoleId) -> Result<()> {
		check_empty(request!(
			self,
			delete,
			"/guilds/{}/members/{}/roles/{}",
			server,
			user,
			role
		))
	}

	/// Edit member information, including roles, nickname, and voice state.
	///
	/// See the `EditMember` struct for the editable fields.
	pub fn edit_member<F: FnOnce(EditMember) -> EditMember>(
		&self,
		server: ServerId,
		user: UserId,
		f: F,
	) -> Result<()> {
		let map = EditMember::__build(f);
		let body = serde_json::to_string(&map)?;
		check_empty(request!(
			self,
			patch(body),
			"/guilds/{}/members/{}",
			server,
			user
		))
	}

	/// Nickname current user.
	///
	/// Similar to `edit_member`
	pub fn edit_nickname(&self, server: ServerId, nick: &str) -> Result<()> {
		let map = json! {{ "nick": nick }};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(
			self,
			patch(body),
			"/guilds/{}/members/@me/nick",
			server
		))
	}

	/// Kick a member from a server.
	pub fn kick_member(&self, server: ServerId, user: UserId) -> Result<()> {
		check_empty(request!(
			self,
			delete,
			"/guilds/{}/members/{}",
			server,
			user
		))
	}

	/// Retrieve the list of roles for a server.
	pub fn get_roles(&self, server: ServerId) -> Result<Vec<Role>> {
		let response = request!(self, get, "/guilds/{}/roles", server);
		decode_array(serde_json::from_reader(response)?, Role::decode)
	}

	/// Create a new role on a server.
	pub fn create_role(
		&self,
		server: ServerId,
		name: Option<&str>,
		permissions: Option<Permissions>,
		color: Option<u64>,
		hoist: Option<bool>,
		mentionable: Option<bool>,
	) -> Result<Role> {
		let map = json! {{
			"name": name,
			"permissions": permissions,
			"color": color,
			"hoist": hoist,
			"mentionable": mentionable,
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds/{}/roles", server);
		Role::decode(serde_json::from_reader(response)?)
	}

	/// Create a new role on a server.
	pub fn create_role_from_builder<F: FnOnce(EditRole) -> EditRole>(
		&self,
		server: ServerId,
		f: F,
	) -> Result<Role> {
		let map = EditRole::__build(f);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds/{}/roles", server);
		Role::decode(serde_json::from_reader(response)?)
	}

	/// Modify a role on a server.
	pub fn edit_role<F: FnOnce(EditRole) -> EditRole>(
		&self,
		server: ServerId,
		role: RoleId,
		f: F,
	) -> Result<Role> {
		let map = EditRole::__build(f);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/guilds/{}/roles/{}", server, role);
		Role::decode(serde_json::from_reader(response)?)
	}

	/// Reorder the roles on a server.
	pub fn reorder_roles(&self, server: ServerId, roles: &[(RoleId, usize)]) -> Result<Vec<Role>> {
		let map: serde_json::Value = roles
			.iter()
			.map(|&(id, pos)| {
				json! {{
					"id": id,
					"position": pos
				}}
			})
			.collect();
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/guilds/{}/roles", server);
		decode_array(serde_json::from_reader(response)?, Role::decode)
	}

	/// Remove specified role from a server.
	pub fn delete_role(&self, server: ServerId, role: RoleId) -> Result<()> {
		check_empty(request!(self, delete, "/guilds/{}/roles/{}", server, role))
	}

	/// Create a private channel with the given user, or return the existing
	/// one if it exists.
	pub fn create_private_channel(&self, recipient: UserId) -> Result<PrivateChannel> {
		let map = json! {{ "recipient_id": recipient }};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/users/@me/channels");
		PrivateChannel::decode(serde_json::from_reader(response)?)
	}

	/// Get the URL at which a user's avatar is located.
	pub fn get_user_avatar_url(&self, user: UserId, avatar: &str) -> String {
		format!(api_concat!("/users/{}/avatars/{}.jpg"), user, avatar)
	}

	/// Download a user's avatar.
	pub fn get_user_avatar(&self, user: UserId, avatar: &str) -> Result<Vec<u8>> {
		use std::io::Read;
		let mut response = retry(|| self.client.get(&self.get_user_avatar_url(user, avatar)))?;
		let mut vec = Vec::new();
		response.read_to_end(&mut vec)?;
		Ok(vec)
	}

	/// Get information about a user.
	/// https://discord.com/developers/docs/resources/user#get-user
	pub fn get_user(&self, user: UserId) -> Result<User> {
		let response = request!(self, get, "/users/{}", user);
		from_reader(response)
	}

	/// Create a new DM channel with a user.
	/// https://discord.com/developers/docs/resources/user#create-dm
	pub fn create_dm(&self, recipient_id: UserId) -> Result<PrivateChannel> {
		let map = json! {{
			"recipient_id": recipient_id.0,
		}};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/users/@me/channels");
		let json: serde_json::Value = from_reader(response)?;
		PrivateChannel::decode(json)
	}

	/// Get the logged-in user's profile.
	pub fn get_current_user(&self) -> Result<CurrentUser> {
		let response = request!(self, get, "/users/@me");
		from_reader(response)
	}

	/// Edit the logged-in bot or user's profile. See `EditProfile` for editable fields.
	///
	/// Usable for bot and user accounts. Only allows updating the username and
	/// avatar.
	pub fn edit_profile<F: FnOnce(EditProfile) -> EditProfile>(&self, f: F) -> Result<CurrentUser> {
		// First, get the current profile, so that providing username and avatar is optional.
		let response = request!(self, get, "/users/@me");
		let user: CurrentUser = from_reader(response)?;
		let mut map = Object::new();
		map.insert("username".into(), json!(user.username));
		map.insert("avatar".into(), json!(user.avatar));

		// Then, send the profile patch.
		let map = EditProfile::__apply(f, map);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/users/@me");
		from_reader(response)
	}

	/// Edit the logged-in non-bot user's profile. See `EditUserProfile` for editable fields.
	///
	/// Usable only for user (non-bot) accounts. Requires mutable access in order
	/// to keep the login token up to date in the event of a password change.
	pub fn edit_user_profile<F: FnOnce(EditUserProfile) -> EditUserProfile>(
		&mut self,
		f: F,
	) -> Result<CurrentUser> {
		// First, get the current profile, so that providing username and avatar is optional.
		let response = request!(self, get, "/users/@me");
		let user: CurrentUser = from_reader(response)?;
		if user.bot {
			return Err(Error::Other(
				"Cannot call edit_user_profile on a bot account",
			));
		}
		let mut map = Object::new();
		map.insert("username".into(), json!(user.username));
		map.insert("avatar".into(), json!(user.avatar));
		if let Some(email) = user.email.as_ref() {
			map.insert("email".into(), email.as_str().into());
		}

		// Then, send the profile patch.
		let map = EditUserProfile::__apply(f, map);
		let body = serde_json::to_string(&map)?;
		let response = request!(self, patch(body), "/users/@me");
		let mut json: Object = serde_json::from_reader(response)?;

		// If a token was included in the response, switch to it. Important because if the
		// password was changed, the old token is invalidated.
		if let Some(serde_json::Value::String(token)) = json.remove("token") {
			self.token = token;
		}
		CurrentUser::decode(serde_json::Value::Object(json))
	}

	/// Get the list of available voice regions for a server.
	pub fn get_voice_regions(&self) -> Result<Vec<VoiceRegion>> {
		let response = request!(self, get, "/voice/regions");
		from_reader(response)
	}

	/// Move a server member to another voice channel.
	pub fn move_member_voice(
		&self,
		server: ServerId,
		user: UserId,
		channel: ChannelId,
	) -> Result<()> {
		let map = json! {{ "channel_id": channel }};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(
			self,
			patch(body),
			"/guilds/{}/members/{}",
			server,
			user
		))
	}

	/// Start a prune operation, kicking members who have been inactive for the
	/// specified number of days. Members with a role assigned will never be
	/// pruned.
	pub fn begin_server_prune(&self, server: ServerId, days: u16) -> Result<ServerPrune> {
		let map = json! {{ "days": days }};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, post(body), "/guilds/{}/prune", server);
		from_reader(response)
	}

	/// Get the number of members who have been inactive for the specified
	/// number of days and would be pruned by a prune operation. Members with a
	/// role assigned will never be pruned.
	pub fn get_server_prune_count(&self, server: ServerId, days: u16) -> Result<ServerPrune> {
		let map = json! {{ "days": days }};
		let body = serde_json::to_string(&map)?;
		let response = request!(self, get(body), "/guilds/{}/prune", server);
		from_reader(response)
	}

	/// Sets a note for the user that is readable only to the currently logged
	/// in user.
	///
	/// This endpoint is only available for users, and so does not work for
	/// bots.
	pub fn edit_note(&self, user: UserId, note: &str) -> Result<()> {
		let map = json! {{ "note": note }};
		let body = serde_json::to_string(&map)?;
		check_empty(request!(self, put(body), "/users/@me/notes/{}", user))
	}

	/// Retrieves information about the application and the owner.
	pub fn get_application_info(&self) -> Result<ApplicationInfo> {
		let response = request!(self, get, "/oauth2/applications/@me");
		from_reader(response)
	}

	/// Retrieves the number of guild shards Discord suggests to use based on
	/// the number of guilds.
	///
	/// This endpoint is only available for bots.
	pub fn suggested_shard_count(&self) -> Result<u64> {
		let response = request!(self, get, "/gateway/bot");
		let mut value: Object = serde_json::from_reader(response)?;
		match value.remove("shards") {
			Some(value) => match value.as_u64() {
				Some(shards) => Ok(shards),
				None => Err(Error::Decode("Invalid \"shards\"", value)),
			},
			None => Err(Error::Decode(
				"suggested_shard_count missing \"shards\"",
				serde_json::Value::Object(value),
			)),
		}
	}

	/// Establish a websocket connection over which events can be received.
	///
	/// Also returns the `ReadyEvent` sent by Discord upon establishing the
	/// connection, which contains the initial state as seen by the client.
	///
	/// See `connect_sharded` if you want to use guild sharding.
	pub fn connect(&self) -> Result<(Connection, ReadyEvent)> {
		self.connection_builder()?.connect()
	}

	/// Establish a sharded websocket connection over which events can be
	/// received.
	///
	/// The `shard_id` is indexed at 0 while `total_shards` is indexed at 1.
	///
	/// Also returns the `ReadyEvent` sent by Discord upon establishing the
	/// connection, which contains the initial state as seen by the client.
	///
	/// See `connect` if you do not want to use guild sharding.
	pub fn connect_sharded(
		&self,
		shard_id: u8,
		total_shards: u8,
	) -> Result<(Connection, ReadyEvent)> {
		self.connection_builder()?.with_shard(shard_id, total_shards).connect()
	}

	/// Prepare to establish a websocket connection over which events can be
	/// received.
	pub fn connection_builder(&self) -> Result<connection::ConnectionBuilder> {
		let url = self.get_gateway_url()?;
		Ok(connection::ConnectionBuilder::new(url, &self.token))
	}

	fn get_gateway_url(&self) -> Result<String> {
		let response = request!(self, get, "/gateway");
		let mut value: BTreeMap<String, String> = serde_json::from_reader(response)?;
		match value.remove("url") {
			Some(url) => Ok(url),
			None => Err(Error::Protocol("Response missing \"url\" in Discord::get_gateway_url()"))
		}
	}
}

fn from_reader<T: serde::de::DeserializeOwned, R: std::io::Read>(r: R) -> Result<T> {
	serde_json::from_reader(r).map_err(From::from)
}

/// Read an image from a file into a string suitable for upload.
///
/// If the file's extension is `.png`, the claimed media type will be `image/png`, or `image/jpg`
/// otherwise. Note that Discord may convert the image to JPEG or another format after upload.
pub fn read_image<P: AsRef<::std::path::Path>>(path: P) -> Result<String> {
	use std::io::Read;
	let path = path.as_ref();
	let mut vec = Vec::new();
	std::fs::File::open(path)?.read_to_end(&mut vec)?;
	Ok(format!(
		"data:image/{};base64,{}",
		if path.extension() == Some("png".as_ref()) {
			"png"
		} else {
			"jpg"
		},
		base64::encode(&vec),
	))
}

/// Retrieves the current unresolved incidents from the status page.
pub fn get_unresolved_incidents() -> Result<Vec<Incident>> {
	let client = tls_client();
	let response = retry(|| client.get(status_concat!("/incidents/unresolved.json")))?;
	let mut json: Object = serde_json::from_reader(response)?;

	match json.remove("incidents") {
		Some(incidents) => decode_array(incidents, Incident::decode),
		None => Ok(vec![]),
	}
}

/// Retrieves the active maintenances from the status page.
pub fn get_active_maintenances() -> Result<Vec<Maintenance>> {
	let client = tls_client();
	let response = check_status(retry(|| {
		client.get(status_concat!("/scheduled-maintenances/active.json"))
	}))?;
	let mut json: Object = serde_json::from_reader(response)?;

	match json.remove("scheduled_maintenances") {
		Some(scheduled_maintenances) => decode_array(scheduled_maintenances, Maintenance::decode),
		None => Ok(vec![]),
	}
}

/// Retrieves the upcoming maintenances from the status page.
pub fn get_upcoming_maintenances() -> Result<Vec<Maintenance>> {
	let client = tls_client();
	let response = check_status(retry(|| {
		client.get(status_concat!("/scheduled-maintenances/upcoming.json"))
	}))?;
	let mut json: Object = serde_json::from_reader(response)?;

	match json.remove("scheduled_maintenances") {
		Some(scheduled_maintenances) => decode_array(scheduled_maintenances, Maintenance::decode),
		None => Ok(vec![]),
	}
}

/// Argument to `get_messages` to specify the desired message retrieval.
pub enum GetMessages {
	/// Get the N most recent messages.
	MostRecent,
	/// Get the first N messages before the specified message.
	Before(MessageId),
	/// Get the first N messages after the specified message.
	After(MessageId),
	/// Get N/2 messages before, N/2 messages after, and the specified message.
	Around(MessageId),
}

/// Send a request with the correct `UserAgent`, retrying it a second time if the
/// connection is aborted the first time.
fn retry<'a, F: Fn() -> hyper::client::RequestBuilder<'a>>(
	f: F,
) -> hyper::Result<hyper::client::Response> {
	let f2 = || {
		f().header(hyper::header::UserAgent(USER_AGENT.to_owned()))
			.send()
	};
	// retry on a ConnectionAborted, which occurs if it's been a while since the last request
	match f2() {
		Err(hyper::error::Error::Io(ref io))
			if io.kind() == std::io::ErrorKind::ConnectionAborted =>
		{
			f2()
		}
		other => other,
	}
}

/// Convert non-success hyper statuses to discord crate errors, tossing info.
fn check_status(
	response: hyper::Result<hyper::client::Response>,
) -> Result<hyper::client::Response> {
	let response: hyper::client::Response = response?;
	if !response.status.is_success() {
		return Err(Error::from_response(response));
	}
	Ok(response)
}

/// Validate a request that is expected to return 204 No Content and print
/// debug information if it does not.
fn check_empty(mut response: hyper::client::Response) -> Result<()> {
	if response.status != hyper::status::StatusCode::NoContent {
		use std::io::Read;
		debug!("Expected 204 No Content, got {}", response.status);
		for header in response.headers.iter() {
			debug!("Header: {}", header);
		}
		let mut content = String::new();
		response.read_to_string(&mut content)?;
		debug!("Content: {}", content);
	}
	Ok(())
}

fn resolve_invite(invite: &str) -> &str {
	if invite.starts_with("http://discord.gg/") {
		&invite[18..]
	} else if invite.starts_with("https://discord.gg/") {
		&invite[19..]
	} else if invite.starts_with("discord.gg/") {
		&invite[11..]
	} else {
		invite
	}
}

fn sleep_ms(millis: u64) {
	std::thread::sleep(time::Duration::from_millis(millis))
}

// Timer that remembers when it is supposed to go off
struct Timer {
	next_tick_at: time::Instant,
	tick_len: time::Duration,
}

#[cfg_attr(not(feature = "voice"), allow(dead_code))]
impl Timer {
	fn new(tick_len_ms: u64) -> Timer {
		let tick_len = time::Duration::from_millis(tick_len_ms);
		Timer {
			next_tick_at: time::Instant::now() + tick_len,
			tick_len: tick_len,
		}
	}

	#[allow(dead_code)]
	fn immediately(&mut self) {
		self.next_tick_at = time::Instant::now();
	}

	fn defer(&mut self) {
		self.next_tick_at = time::Instant::now() + self.tick_len;
	}

	fn check_tick(&mut self) -> bool {
		if time::Instant::now() >= self.next_tick_at {
			self.next_tick_at = self.next_tick_at + self.tick_len;
			true
		} else {
			false
		}
	}

	fn sleep_until_tick(&mut self) {
		let now = time::Instant::now();
		if self.next_tick_at > now {
			std::thread::sleep(self.next_tick_at - now);
		}
		self.next_tick_at = self.next_tick_at + self.tick_len;
	}
}

trait ReceiverExt {
	fn recv_json<F, T>(&mut self, decode: F) -> Result<T>
	where
		F: FnOnce(serde_json::Value) -> Result<T>;
}

trait SenderExt {
	fn send_json(&mut self, value: &serde_json::Value) -> Result<()>;
}

impl ReceiverExt for websocket::client::Receiver<websocket::stream::WebSocketStream> {
	fn recv_json<F, T>(&mut self, decode: F) -> Result<T>
	where
		F: FnOnce(serde_json::Value) -> Result<T>,
	{
		use websocket::message::{Message, Type};
		use websocket::ws::receiver::Receiver;
		let message: Message = self.recv_message()?;
		if message.opcode == Type::Close {
			Err(Error::Closed(
				message.cd_status_code,
				String::from_utf8_lossy(&message.payload).into_owned(),
			))
		} else if message.opcode == Type::Binary || message.opcode == Type::Text {
			let mut payload_vec;
			let payload = if message.opcode == Type::Binary {
				use std::io::Read;
				payload_vec = Vec::new();
				flate2::read::ZlibDecoder::new(&message.payload[..])
					.read_to_end(&mut payload_vec)?;
				&payload_vec[..]
			} else {
				&message.payload[..]
			};
			serde_json::from_reader(payload)
				.map_err(From::from)
				.and_then(decode)
				.map_err(|e| {
					warn!("Error decoding: {}", String::from_utf8_lossy(payload));
					e
				})
		} else {
			Err(Error::Closed(
				None,
				String::from_utf8_lossy(&message.payload).into_owned(),
			))
		}
	}
}

impl SenderExt for websocket::client::Sender<websocket::stream::WebSocketStream> {
	fn send_json(&mut self, value: &serde_json::Value) -> Result<()> {
		use websocket::message::Message;
		use websocket::ws::sender::Sender;
		serde_json::to_string(value)
			.map(Message::text)
			.map_err(Error::from)
			.and_then(|m| self.send_message(&m).map_err(Error::from))
	}
}

mod internal {
	pub enum Status {
		SendMessage(::serde_json::Value),
		Sequence(u64),
		ChangeInterval(u64),
		ChangeSender(::websocket::client::Sender<::websocket::stream::WebSocketStream>),
		Aborted,
	}
}
