# Lavalink Guide

This repository does not expose Lavalink as a live runtime backend in the public template.

`Songbird` is the implemented backend. This document exists only as an operational reference for a future external-node path if you decide to maintain a private fork or add another provider later.

## When To Consider Lavalink

- You want music processing isolated from the main bot process.
- You expect enough concurrent playback that a separate media node is useful.
- You are comfortable operating a Java service or container alongside the bot.

## External Node Inputs You Would Typically Need

- host
- port
- password
- secure flag
- optional resume key / session label

## Deployment Shape

1. Run Lavalink as a separate service.
2. Keep the Rust bot process separate.
3. Treat the Rust side as a client only.
4. Do not assume the public template dashboard will manage the node.

## Why It Is Documentation-Only Here

- The public template is optimized for a lighter default setup.
- `Songbird + yt-dlp` is sufficient for the intended small-scale deployment profile.
- Exposing an unfinished external node path in the UI would create misleading operational complexity.
