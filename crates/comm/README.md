# nebo-comm

The Rust client for the NeboAI hub. It is what connects a bot to NeboAI, and
it is the one implementation used by Nebo itself and by Nebo Link.

It covers three things:

- **Pairing.** `api::redeem_code` trades a connect code from the owner's
  NeboAI account for the bot's credentials (`POST /api/v1/bots/connect/redeem`).
- **The comms connection.** `NeboAIPlugin` holds the bot's outbound WebSocket
  to the hub: the binary frame protocol (`frame`, `wire`), presence, message
  delivery with acked offsets, token rotation and the bot lease (`lease`).
- **The management tunnel.** `tunnel::run` dials the hub over an outbound
  WebSocket and serves hub-opened yamux streams to a local address, so the
  owner can reach the bot from NeboAI without anything listening publicly.

`api::NeboAIApi` is the authenticated REST client for the rest of the hub API.

## License

Apache-2.0
