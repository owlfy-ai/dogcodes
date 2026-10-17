# Native attachment input verification

Implementation and real-driver acceptance are now recorded in [attachment-delivery-2026-09-14.md](attachment-delivery-2026-09-14.md). The investigation below preserves the evidence gathered before implementation.
This is a protocol investigation, not an implemented attachment delivery change.

## Required behavior

Images use native inline image data whenever supported, including clipboard screenshots without a user-supplied filesystem path. Ordinary files use file references. Prompt text, attachment metadata and image content must be separate throughout submission and history rendering. App-owned caching for persistence does not imply sending an image path to the Agent.

## Verified local interfaces

- Kimi 0.42.0: real ACP initialize advertises image=true, embeddedContext=true.
- Cursor 2026.09.10-fd3934a: real ACP initialize advertises image=true, embeddedContext=false.
- CodeBuddy installed CLI: real ACP initialize advertises image=true, embeddedContext=true.
- Grok Build 1.0.30: real ACP initialize advertises image=false, embeddedContext=true. Follow-up source inspection and a real inline-image prompt proved that this version nevertheless accepts ACP image blocks and correctly recognizes their pixels. The earlier handshake-only conclusion was too conservative; see follow-up below.
- ACP image shape: {type:"image", mimeType:"image/png", data:"<base64>"}. Ordinary files use {type:"resource_link",uri:"file:///...",name:"...",mimeType:"..."}. Resource links are references, not proof that the Agent decoded a binary file or saw an image.
- Codex: installed app-server generate-json-schema exposes image {type:"image",url:string} and localImage {type:"localImage",path:string} in TurnStartParams. Inline data URLs belong in image.url, not localImage.path. A generic native file input was not established by this check.
- Claude Code: Agent SDK streaming input documents image blocks with source {type:"base64",media_type,data}; the current local driver sends a user.message.content array, but currently creates text blocks only. Actual visual comprehension was not exercised in this investigation.
- ZCode: installed /Applications/ZCode.app/Contents/Resources/glm/zcode.cjs validates session/send with a separate attachments array. Its SLi/kLi conversion reads kind, filename, mimeType, sizeBytes, localPath, dataBase64 and textContent. Images without localPath become a data URL when kind="image" and dataBase64 is supplied. Files with kind="file" and localPath become native file attachments. Avoid supplying image localPath because it takes precedence over dataBase64.
- Antigravity agy 1.2.2: real stream-json prompt with an image content block was rejected before a model turn with: `stream input content block type "image" is not supported (only "text")`. The process returned an ERROR result with num_turns=0. Test used a bundled application icon in an isolated temporary directory; it requested no tools. App/history support for images must not be confused with CLI stdin support. Current ordinary file references can only travel as separate text blocks, not as a claimed native file attachment.

## Evidence

- ACP handshake results: /tmp/attachment-protocol-caps-final.log (fresh initialized connections; no prompts).
- Antigravity actual rejection: /tmp/agy-image-result.log.
- Installed Codex generated schema: /tmp/dogcodes-codex-attachment-schema/v2/TurnStartParams.json.
- ACP v1 content: https://agentclientprotocol.com/protocol/v1/content
- Claude SDK streaming images: https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode

These results establish declared capabilities/schema acceptance boundaries, not end-to-end image comprehension on every model. Implementation must negotiate capability at runtime, preserve user text without attachment JSON, keep attachments across failed submissions, and surface unsupported image input explicitly instead of silently dropping it or substituting a path. Native file-reference reading still depends on path accessibility and approval policy.


## Follow-up: Grok and Antigravity alternatives

### Grok Build 1.0.30 (04b7ffed98c6)

Official source inspected at xai-org/grok-build commit 37949780c144e37df692e3d669051a21fec24f20:

- `crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs:542` initializes prompt capabilities with embedded_context(true), omitting image(true).
- `crates/codegen/xai-grok-shell/src/session/prompt_parser.rs:96` actually accepts ContentBlock::Image and returns those images with the parsed prompt.
- `crates/codegen/xai-grok-tools/src/implementations/grok_build/read_file/mod.rs:440` can return decoded image data from read_file too.
- CLI help and `14-headless-mode.md` expose --prompt-json for JSON content blocks, but changing to one-shot CLI transport is unnecessary for the tested ACP version.

Real test: fresh `grok agent --no-leader stdio`, fresh session in an empty disposable directory. The client sent only text plus inline image/png Base64 bytes from the bundled Kimi icon. No image file or path was placed in the Agent workspace or supplied to the Agent. Native tool requests were disabled/denied by the probe. The response correctly identified a white K on a black background; end_turn succeeded, and no tool events occurred. This is a confirmed native direct-image path despite the advertised capability omission, not merely evidence that a text prompt was accepted.

Production handling should isolate this as a tested, version-specific Grok compatibility rule, retain normal advertised capability checks for other ACP providers, and surface actual rejections. This does not establish image support for every Grok model or future release.

### Antigravity agy 1.2.2

The official headless docs explicitly restrict stream-json to text blocks. The previous inline-image rejection remains accurate.

A viable alternative was verified: app-owned PNG cache -> file reference in a separate text block -> native view_file -> model image comprehension. First attempt without a declared project directory auto-denied the file read. Retest used a canonical disposable directory, --new-project and --add-dir <directory>, with standard request-review permissions. No global settings or credentials were changed and --dangerously-skip-permissions was not used. view_file completed and the answer identified the white K, blue mark and black textured background in the actual pixels.

For production, retain cached files long enough for execution/history and make their exact directory accessible under the existing workspace and approval policy; do not widen all filesystem permissions. Clipboard users need not save screenshots themselves. Keep the file reference outside displayed user prompt prose. This is a tool-mediated image path, not native inline image submission, and adds one tool round trip.

No supported public alternative inline input route was established by the current CLI help and official documentation. App-private RPC/GUI automation or a separate vision API would require a different integration and were not proven here.

### Reproducible evidence

Compact receipts are in `attachment-fallback-receipts-2026-09-14.json`; raw local logs: `/tmp/grok-image-direct-result.log`, `/tmp/agy-read-image-workspace-result.log`. Test scripts: `/tmp/probe-grok-image-direct.py`, `/tmp/probe-agy-read-image-workspace.py`.

Sources:
- https://github.com/xai-org/grok-build/blob/37949780c144e37df692e3d669051a21fec24f20/crates/codegen/xai-grok-shell/src/session/prompt_parser.rs#L96
- https://github.com/xai-org/grok-build/blob/37949780c144e37df692e3d669051a21fec24f20/crates/codegen/xai-grok-shell/src/agent/mvp_agent/acp_agent.rs#L542
- https://www.antigravity.google/docs/cli/headless/#send-a-prompt
