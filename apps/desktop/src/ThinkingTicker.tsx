import { translate as t } from "./i18n";
import { useContext, useEffect, useState } from "react";
import type { CoreClient } from "@dogcodes/protocol";

import { ConversationPageVisible } from "./ConversationPages";

export function ThinkingTicker({ text, embedded = false, itemId, client }: {
  text?: string; embedded?: boolean; itemId: string; client?: CoreClient;
}) {
  const [preview, setPreview] = useState<string | null>(null);
  const visible = useContext(ConversationPageVisible);
  useEffect(() => {
    if (!visible || !client) return;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    const sample = async () => {
      if (!client.liveThoughtPreview) return;
      try {
        const next = await client.liveThoughtPreview(itemId);
        if (!disposed) setPreview(next);
      } catch { /* Keep the previous preview across transient read failures. */ }
      if (!disposed) timer = setTimeout(sample, 200);
    };
    void sample();
    return () => { disposed = true; clearTimeout(timer); };
  }, [client, itemId, visible]);
  if (!(preview ?? text)) return null;
  return (
    <span className="thought-marquee" tabIndex={embedded ? undefined : 0} aria-label={t("activity.thinking")}>
      <span aria-hidden="true">{preview ?? text}</span>
    </span>
  );
}
