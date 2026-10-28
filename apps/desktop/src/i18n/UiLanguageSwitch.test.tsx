// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../App";
import { DemoCoreClient } from "../demoClient";
import { UI_LANGUAGE_KEY } from "./index";

let root: Root, element: HTMLDivElement, client: DemoCoreClient;

const click = async (target: Element) => {
  await act(async () => {
    (target as HTMLElement).click();
  });
};

describe("UiLanguage switching across international languages", () => {
  beforeEach(async () => {
    localStorage.clear();
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.stubGlobal("ResizeObserver", class { observe() {} disconnect() {} });
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
    vi.stubGlobal("cancelAnimationFrame", clearTimeout);
    vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
    HTMLElement.prototype.scrollIntoView = vi.fn();
    client = new DemoCoreClient();
    element = document.createElement("div");
    document.body.append(element);
    root = createRoot(element);
    await act(async () => {
      root.render(<App client={client} />);
    });
  });

  afterEach(async () => {
    await act(async () => {
      root.unmount();
    });
    element.remove();
    vi.useRealTimers();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("switches interface language to English", async () => {
    const trigger = element.querySelector<HTMLButtonElement>(".preferences-trigger")!;
    expect(trigger).toBeTruthy();
    await click(trigger);

    const langMenuItem = [...document.body.querySelectorAll<HTMLButtonElement>(".preferences-popover button")].find(
      (b) => b.textContent?.includes("界面语言") || b.textContent?.includes("Language")
    )!;
    expect(langMenuItem).toBeTruthy();
    await click(langMenuItem);

    const enOption = [...document.body.querySelectorAll<HTMLButtonElement>('.preferences-submenu button')].find(
      (b) => b.textContent?.includes("English")
    )!;
    expect(enOption).toBeTruthy();
    await click(enOption);

    expect(localStorage.getItem(UI_LANGUAGE_KEY)).toBe("en");
    expect(document.documentElement.lang).toBe("en");

    expect(element.textContent).toContain("New conversation");
    expect(element.textContent).toContain("Agent Management");
    const navigation = element.querySelector(".primary-nav")!;
    const entries = [...navigation.querySelectorAll("button")];
    expect(entries.map(button => button.textContent)).toEqual([expect.stringContaining("New conversation"), expect.stringContaining("Agent Management"), "Connect phone"]);
    expect(entries[2].getAttribute("aria-haspopup")).toBe("dialog");
    await click(trigger);
    expect(document.querySelector(".preferences-popover")?.textContent).not.toContain("Connect phone");
  });

  it("switches interface language to Japanese", async () => {
    const trigger = element.querySelector<HTMLButtonElement>(".preferences-trigger")!;
    await click(trigger);

    const langMenuItem = [...document.body.querySelectorAll<HTMLButtonElement>(".preferences-popover button")].find(
      (b) => b.textContent?.includes("界面语言") || b.textContent?.includes("Language")
    )!;
    await click(langMenuItem);

    const jaOption = [...document.body.querySelectorAll<HTMLButtonElement>('.preferences-submenu button')].find(
      (b) => b.textContent?.includes("日本語")
    )!;
    expect(jaOption).toBeTruthy();
    await click(jaOption);

    expect(localStorage.getItem(UI_LANGUAGE_KEY)).toBe("ja");
    expect(document.documentElement.lang).toBe("ja");

    expect(element.textContent).toContain("新規チャット");
    expect(element.textContent).toContain("Agent 管理");
    expect(element.querySelector(".primary-nav")?.textContent).toContain("スマートフォンを接続");
  });

  it("switches interface language to French", async () => {
    const trigger = element.querySelector<HTMLButtonElement>(".preferences-trigger")!;
    await click(trigger);

    const langMenuItem = [...document.body.querySelectorAll<HTMLButtonElement>(".preferences-popover button")].find(
      (b) => b.textContent?.includes("界面语言") || b.textContent?.includes("Language")
    )!;
    await click(langMenuItem);

    const frOption = [...document.body.querySelectorAll<HTMLButtonElement>('.preferences-submenu button')].find(
      (b) => b.textContent?.includes("Français")
    )!;
    expect(frOption).toBeTruthy();
    await click(frOption);

    expect(localStorage.getItem(UI_LANGUAGE_KEY)).toBe("fr");
    expect(document.documentElement.lang).toBe("fr");

    expect(element.textContent).toContain("Nouvelle conversation");
  });

  it("switches interface language to Traditional Chinese", async () => {
    const trigger = element.querySelector<HTMLButtonElement>(".preferences-trigger")!;
    await click(trigger);

    const langMenuItem = [...document.body.querySelectorAll<HTMLButtonElement>(".preferences-popover button")].find(
      (b) => b.textContent?.includes("界面语言") || b.textContent?.includes("Language")
    )!;
    await click(langMenuItem);

    const twOption = [...document.body.querySelectorAll<HTMLButtonElement>('.preferences-submenu button')].find(
      (b) => b.textContent?.includes("繁體中文")
    )!;
    expect(twOption).toBeTruthy();
    await click(twOption);

    expect(localStorage.getItem(UI_LANGUAGE_KEY)).toBe("zh-TW");
    expect(document.documentElement.lang).toBe("zh-TW");

    expect(element.textContent).toContain("新增對話");
  });
});
