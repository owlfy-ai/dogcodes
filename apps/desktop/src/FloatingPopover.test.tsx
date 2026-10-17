// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { FloatingPopover } from "./FloatingPopover";

it('uses the top layer, clamps to viewport and hides on close', async () => {
  vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
  vi.stubGlobal('innerWidth', 400); vi.stubGlobal('innerHeight', 300);
  vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  const shown = new WeakSet<HTMLElement>();
  const matches = HTMLElement.prototype.matches;
  vi.spyOn(HTMLElement.prototype, 'matches').mockImplementation(function (this: HTMLElement, selector) { return selector === ':popover-open' ? shown.has(this) : matches.call(this, selector); });
  Object.defineProperty(HTMLElement.prototype, 'showPopover', { configurable: true, value: function(this: HTMLElement) { shown.add(this); } });
  Object.defineProperty(HTMLElement.prototype, 'hidePopover', { configurable: true, value: function(this: HTMLElement) { shown.delete(this); } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) { return this.className === 'floating' ? new DOMRect(0, 0, 280, 100) : new DOMRect(0, 160, 100, 20); });
  const host = document.createElement('div'); document.body.append(host); const root = createRoot(host);
  try {
    await act(async () => root.render(<FloatingPopover className="floating">Content</FloatingPopover>));
    const node = host.firstElementChild as HTMLElement;
    expect(shown.has(node)).toBe(true); expect(node.getAttribute('popover')).toBe('manual');
    expect(node.style.position).toBe('fixed'); expect(node.style.left).toBe('8px'); expect(node.style.top).toBe('52px');
    // Near the top, move below the trigger rather than under a clipping ancestor.
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) { return this.className === 'floating' ? new DOMRect(0, 0, 280, 100) : new DOMRect(0, 12, 100, 20); });
    await act(async () => window.dispatchEvent(new Event('scroll')));
    expect(node.style.top).toBe('40px');
    expect(node.style.maxHeight).toBe('252px');
    // Fractional anchors share the same device-pixel alignment above and below.
    vi.stubGlobal('devicePixelRatio', 2);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) { return this.className === 'floating' ? new DOMRect(0, 0, 280, 100) : new DOMRect(200.3, 12.3, 100, 20); });
    await act(async () => window.dispatchEvent(new Event('resize')));
    expect(node.style.left).toBe('20.5px'); expect(node.style.top).toBe('40.5px');
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) { return this.className === 'floating' ? new DOMRect(0, 0, 280, 100) : new DOMRect(200.3, 160.3, 100, 20); });
    await act(async () => window.dispatchEvent(new Event('resize')));
    expect(node.style.left).toBe('20.5px'); expect(node.style.top).toBe('52.5px');
    // An oversized menu scrolls on the roomier side and leaves the trigger visible.
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) { return this.className === 'floating' ? new DOMRect(0, 0, 280, 500) : new DOMRect(0, 210, 100, 20); });
    await act(async () => window.dispatchEvent(new Event('resize')));
    expect(node.style.top).toBe('8px');
    expect(node.style.maxHeight).toBe('194px');
    expect(node.style.overflow).toBe('auto');
    // Scrolling the menu or a nested region must not measure/reposition it.
    const measure = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect');
    measure.mockClear();
    node.scrollTop = 120;
    await act(async () => node.dispatchEvent(new Event('scroll')));
    const child = document.createElement('div'); node.append(child);
    await act(async () => child.dispatchEvent(new Event('scroll')));
    expect(measure).not.toHaveBeenCalled();
    expect(node.scrollTop).toBe(120);
    // Simulate the browser clamping scroll when the measurement removes overflow.
    measure.mockImplementation(function(this: HTMLElement) {
      if (this === node) { node.scrollTop = 0; node.scrollLeft = 0; return new DOMRect(0, 0, 280, 500); }
      return new DOMRect(0, 210, 100, 20);
    });
    node.scrollLeft = 25;
    await act(async () => window.dispatchEvent(new Event('resize')));
    expect(measure).toHaveBeenCalled();
    expect(node.scrollTop).toBe(120);
    expect(node.scrollLeft).toBe(25);
    await act(async () => root.render(<FloatingPopover className="floating" open={false}>Content</FloatingPopover>));
    expect(shown.has(node)).toBe(false); expect(node.style.display).toBe('none');
  } finally {
    await act(async () => root.unmount()); host.remove(); vi.restoreAllMocks(); vi.unstubAllGlobals();
    Reflect.deleteProperty(HTMLElement.prototype, 'showPopover'); Reflect.deleteProperty(HTMLElement.prototype, 'hidePopover');
  }
});
