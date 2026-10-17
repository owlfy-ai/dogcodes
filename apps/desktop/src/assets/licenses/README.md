# Agent icons

`pi.svg` and `opencode.png` originate from the corresponding `packages/adapters/*/assets/icon.*` in [codex-host](https://github.com/BytePioneer-AI/codex-host). Comparison on 2026-10-16 used commit `e61d25c7d7c41391ea32b5ace05be03dffc5e6cd` and verified the reference files against that Git object:

| Local asset | Pinned reference | Local changes |
| --- | --- | --- |
| `pi.svg` | [Pi source](https://github.com/BytePioneer-AI/codex-host/blob/e61d25c7d7c41391ea32b5ace05be03dffc5e6cd/packages/adapters/pi/assets/icon.svg) | Only `viewBox` changed from `0 0 800 800` to `120 120 560 560`, cropping the viewport; paths are unchanged. |
| `opencode.png` | [OpenCode source](https://github.com/BytePioneer-AI/codex-host/blob/e61d25c7d7c41391ea32b5ace05be03dffc5e6cd/packages/adapters/opencode/assets/icon.png) | Byte-identical. |

The reference repository's [LICENSE](https://github.com/BytePioneer-AI/codex-host/blob/e61d25c7d7c41391ea32b5ace05be03dffc5e6cd/LICENSE) is preserved byte-for-byte in [codex-host-LICENSE.txt](codex-host-LICENSE.txt). It contains LGPL v3; it is not replaced by this project's Apache-2.0 license. The comparison establishes file provenance, not independent permission for every product mark. Upstream asset-specific terms and complete distribution notices remain release review items.

SHA-256 of current local assets:

```text
pi.svg        aaba902b06802cdecbec16e33b7656c4325d0fc31d21690c2202d6c0a55699e6
opencode.png  2f2d5687a641f7fc1c3931072249a5984660375c091064fe8b374e4e43a8656c
```

`kiro.svg` is the unmodified SVG supplied by the user for the Kiro logo.

`deepseek-harness.png` is the unmodified DSH logo from the [GitHub avatar URL supplied by the user](https://avatars.githubusercontent.com/u/148330874?s=60&v=4). These product marks belong to their respective owners.

The Kiro and DSH statements above record how the files were supplied; permission and immutable upstream evidence have not yet been established by this inventory. The remaining Agent artwork (`antigravity.svg`, `chatgpt.svg`, `claude-code.svg`, `codebuddy.svg`, `cursor.svg`, `grok.svg`, `kimi.svg`, `zcode.svg`, and the legacy `kimi.png`) also needs a recorded origin and applicable terms before release. Visual similarity or a matching SVG path alone is not treated as proof of origin. `agentPresentation.tsx` additionally creates light-theme color variants for ChatGPT, Cursor, Grok and ZCode at runtime.
