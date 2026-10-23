# Vendored tiny_http

`tiny_http/` contains the library source, manifest, README, and licenses from
the crates.io `tiny_http` 0.12.0 package. The original archive SHA-256 is
`389915df6413a2e74fb181895f933386023c71110878cd0825588928e64cdc82`
(upstream commit `212b1c45852fef2093dc1374875a9393c55eb4b9`).

The only library change is in `src/util/equal_reader.rs`: unread fixed-length
request bodies are drained with a fixed 8 KiB buffer instead of allocating the
entire remaining `Content-Length`. Otherwise an HTTP 413 response could still
trigger an attacker-controlled allocation while the rejected request is dropped.
The existing drain behavior is preserved, including framing of later requests.

The bridge's HTTP regression test covers Content-Length, chunked bodies, and
a headers-only request declaring `usize::MAX` bytes. Keep this correction until
an upstream release with bounded cleanup can replace the vendored library.
