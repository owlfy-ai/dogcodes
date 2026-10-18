import { useLayoutEffect, useRef, type CSSProperties, type ReactNode } from "react";

export type HerdTone = "installed" | "scanning" | "failed" | "missing";

// A meadow seen from the side. Positions are fractions of the herd width from the
// origin; y grows toward the viewer, so nearer animals are drawn larger and in front.
const depthScale = (y: number) => 1 + y * .6;
const ease = (value: number) => (1 - Math.cos(Math.PI * Math.min(1, Math.max(0, value)))) / 2;
const dogHalfWidth = .075, dogHeight = .1;
const trotSpeed = .085, eyePause = 1.6, restPause = 2.2, restEvery = 2;

/** Every supported agent keeps one slot from the start, so nothing reflows or jumps. */
export function herdLayout(count: number) {
  const columns = Math.max(1, Math.min(5, Math.ceil(count / 3)));
  const rows = Math.max(1, Math.ceil(count / columns));
  const size = count <= 6 ? .12 : .105;
  // The flock stands toward the back of the meadow so the dog can pass in front without covering it.
  const spacing = size * 1.12, rowGap = size * .56, offset = -.05;
  const slots: [number, number][] = [];
  for (let row = 0; row < rows; row++) {
    const inRow = Math.min(columns, count - slots.length);
    // Staggered rows keep each sheep's tag clear of the sheep in front of it.
    const shift = rows > 1 ? (row % 2 ? .25 : -.25) * size : 0;
    for (let col = 0; col < inRow; col++) {
      slots.push([(col - (inRow - 1) / 2) * spacing + shift, (row - (rows - 1) / 2) * rowGap + offset]);
    }
  }
  const halfX = Math.max(0, ...slots.map(([x]) => Math.abs(x))) + size / 2;
  const back = Math.min(...slots.map(([, y]) => y)), front = Math.max(...slots.map(([, y]) => y));
  // The dog's loop keeps its whole body outside the flock: beside it, behind its heads
  // (half hidden there, as a real dog would be) and far enough in front that it never covers a sheep.
  const cy = (back + front) / 2;
  const loop = { cx: 0, cy, a: halfX + dogHalfWidth + .04, back: cy - back + dogHeight * .55, front: front - cy + dogHeight * 1.15 };
  return { slots, size, loop };
}

/** The welcome hero: every agent waits in the flock, and the sheepdog circles it,
 * stopping to eye each agent as it is found. */
export function WelcomeHerd({ agents, renderIcon }: { agents: { id: string; tone: HerdTone }[]; renderIcon: (id: string) => ReactNode }) {
  const herd = useRef<HTMLDivElement>(null);
  const dogRef = useRef<HTMLDivElement>(null);
  const layout = herdLayout(agents.length);
  const ready = useRef(new Set<string>());
  const pending = useRef<string[]>([]);
  // New arrivals get a visit; agents already known at first render do not.
  const known = useRef<Set<string> | null>(null);
  if (known.current === null) known.current = new Set(agents.filter(a => a.tone === "installed").map(a => a.id));
  for (const agent of agents) {
    if (agent.tone === "installed" && !ready.current.has(agent.id)) {
      ready.current.add(agent.id);
      if (!known.current.has(agent.id) && !pending.current.includes(agent.id)) pending.current.push(agent.id);
    } else if (agent.tone !== "installed") {
      // Found again after a rescan counts as a new arrival.
      ready.current.delete(agent.id);
      known.current.delete(agent.id);
    }
  }

  useLayoutEffect(() => {
    const root = herd.current, dog = dogRef.current;
    if (!root || !dog) return;
    const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const { loop } = herdLayout(root.querySelectorAll(".welcome-sheep").length);
    const slotOf = (id: string) => {
      const sheep = [...root.querySelectorAll<HTMLElement>(".welcome-sheep")].find(item => item.dataset.agent === id);
      return sheep ? [Number(sheep.dataset.x), Number(sheep.dataset.y)] as const : null;
    };
    const semi = (angle: number) => Math.sin(angle) < 0 ? loop.back : loop.front;
    const angleOf = ([x, y]: readonly [number, number]) => Math.atan2((y - loop.cy) / (y < loop.cy ? loop.back : loop.front), (x - loop.cx) / loop.a);
    const at = (angle: number) => [loop.cx + loop.a * Math.cos(angle), loop.cy + semi(angle) * Math.sin(angle)] as const;
    const wrap = (angle: number) => ((angle % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI);

    // Start resting behind the flock, then trot clockwise as seen from above.
    let angle = -Math.PI / 2, direction = 1, speed = 0, face = 1, travelled = 0;
    let pause = still ? Infinity : 1.2, pauseKind: "eye" | "down" = "down", target: number | null = null;
    let last = performance.now();
    const mark = (element: HTMLElement, key: string, value: string) => { if (element.dataset[key] !== value) element.dataset[key] = value; };
    const place = (element: HTMLElement, x: number, y: number) => {
      element.style.setProperty("--x", x.toFixed(4));
      element.style.setProperty("--y", y.toFixed(4));
      element.style.setProperty("--depth", depthScale(y).toFixed(3));
      element.style.zIndex = `${Math.round((y + 1) * 100)}`;
    };

    const tick = (now: number) => {
      const elapsed = Math.min(.05, (now - last) / 1000);
      last = now;
      if (target === null && pending.current.length) {
        const slot = slotOf(pending.current[0]);
        if (slot) target = wrap(angleOf(slot)); else pending.current.shift();
      }
      if (pause > 0) {
        pause -= elapsed;
        speed = 0;
      } else {
        // Ease into and out of a stop at the target; otherwise trot steadily.
        let remaining = Infinity;
        if (target !== null) remaining = wrap(direction * (target - angle));
        const [dx, dy] = [-loop.a * Math.sin(angle), semi(angle) * Math.cos(angle)];
        const radius = Math.hypot(dx, dy);
        const distance = remaining * radius;
        const desired = target !== null ? Math.min(trotSpeed, Math.max(.012, distance * 1.4)) : trotSpeed;
        speed += (desired - speed) * Math.min(1, elapsed * 2.5);
        const step = speed * elapsed / radius;
        if (target !== null && step >= remaining) {
          angle = target;
          target = null;
          pending.current.shift();
          pause = eyePause; pauseKind = "eye";
        } else {
          angle = wrap(angle + direction * step);
          travelled += step;
          if (target === null && travelled > restEvery * 2 * Math.PI) {
            travelled = 0; pause = restPause; pauseKind = "down";
          }
        }
      }

      const [x, y] = at(angle);
      place(dog, x, y);
      // Facing and view follow the actual direction of travel; at rest the dog looks at the flock.
      const vx = -direction * loop.a * Math.sin(angle), vy = direction * semi(angle) * Math.cos(angle);
      let view = "side";
      if (pause > 0) {
        if (Math.abs(Math.cos(angle)) > .45) face = x > loop.cx ? -1 : 1;
        else view = y < loop.cy ? "front" : "back";
      } else {
        if (Math.abs(vx) > .002) face = vx > 0 ? 1 : -1;
        // Depth reads larger on screen than it measures, so weigh it before choosing a view.
        if (Math.abs(vy) * 1.6 > Math.abs(vx)) view = vy > 0 ? "front" : "back";
      }
      dog.style.setProperty("--face", `${face}`);
      mark(dog, "view", view);
      mark(dog, "gait", pause > 0 ? pauseKind : speed > trotSpeed * .5 ? "run" : "drive");

      // Sheep the dog passes close by turn away from it; the rest keep grazing.
      root.querySelectorAll<HTMLElement>(".welcome-sheep").forEach(sheep => {
        const sx = Number(sheep.dataset.x), sy = Number(sheep.dataset.y);
        const near = Math.hypot((x - sx) / 1.4, y - sy) < .11;
        const turn = near ? (x > sx ? -1 : 1) : Number(sheep.dataset.home);
        if (sheep.style.getPropertyValue("--facing") !== `${turn}`) sheep.style.setProperty("--facing", `${turn}`);
      });
    };

    tick(performance.now());
    let frame = requestAnimationFrame(function step(now) { tick(now); frame = requestAnimationFrame(step); });
    return () => cancelAnimationFrame(frame);
  }, []);

  return <div className="welcome-herd" ref={herd} aria-hidden="true">
    <div className="welcome-herd-meadow" />
    {agents.map((agent, index) => {
      const [x, y] = layout.slots[index];
      const home = index % 3 === 1 ? -1 : 1;
      const style = {
        "--x": x, "--y": y, "--size": layout.size, "--depth": depthScale(y), "--facing": home,
        "--graze": `${(index * -1.37) % 5}s`, zIndex: Math.round((y + 1) * 100),
      } as CSSProperties;
      const lit = agent.tone === "installed";
      return <span key={agent.id} style={style} data-agent={agent.id} data-x={x} data-y={y} data-home={home}
        className={`welcome-sheep ${lit ? "ready" : "unknown"} ${agent.tone}`}>
        <span className="welcome-sheep-body">
          <SheepShape />
          <span className="welcome-sheep-tag">{lit ? renderIcon(agent.id) : <span className="welcome-sheep-unknown">?</span>}</span>
        </span>
      </span>;
    })}
    <div className="welcome-herd-dog" ref={dogRef}>
      <SheepdogSide /><SheepdogFront /><SheepdogBack />
    </div>
  </div>;
}

function Wool() {
  return <>
    <circle cx="16" cy="21" r="9" /><circle cx="26" cy="14" r="10" /><circle cx="38" cy="15" r="9.5" /><circle cx="45" cy="25" r="8" />
    <circle cx="36" cy="32" r="9" /><circle cx="23" cy="32" r="9" /><circle cx="12" cy="29" r="7" /><ellipse cx="30" cy="24" rx="17" ry="12" />
  </>;
}

function SheepShape() {
  return <svg className="welcome-sheep-shape" viewBox="0 0 60 48">
    <g className="welcome-sheep-legs"><rect x="15" y="32" width="4" height="12" rx="2" /><rect x="23" y="33" width="4" height="12" rx="2" /><rect x="35" y="33" width="4" height="12" rx="2" /><rect x="43" y="32" width="4" height="12" rx="2" /></g>
    <g className="welcome-sheep-wool outline"><Wool /></g>
    <g className="welcome-sheep-wool"><Wool /></g>
    <g className="welcome-sheep-head">
      <ellipse className="welcome-sheep-face" cx="53" cy="21" rx="6" ry="7.5" />
      <ellipse className="welcome-sheep-face" cx="49" cy="15" rx="3.6" ry="1.8" transform="rotate(-25 49 15)" />
      <circle className="welcome-sheep-eye" cx="55" cy="19.5" r="1.1" />
    </g>
  </svg>;
}

function SheepdogSide() {
  return <svg className="welcome-dog-shape side" viewBox="0 0 72 48">
    <g className="welcome-dog-leg back far"><line x1="24" y1="27" x2="21" y2="40" /><circle cx="21" cy="40" r="2.2" /></g>
    <g className="welcome-dog-leg front far"><line x1="45" y1="28" x2="48" y2="40" /><circle cx="48" cy="40" r="2.2" /></g>
    <g className="welcome-dog-tail"><path className="welcome-dog-coat" d="M20 21C13 18 8 14 3 9c1 7 7 13 15 16z" /><circle className="welcome-dog-white" cx="4.5" cy="10.5" r="2.6" /></g>
    <ellipse className="welcome-dog-coat" cx="34" cy="24" rx="16" ry="8" />
    <ellipse className="welcome-dog-white" cx="46" cy="27" rx="5" ry="6" />
    <g className="welcome-dog-head">
      <path className="welcome-dog-coat" d="M44 22l6-9 8 2-4 11z" />
      <circle className="welcome-dog-coat" cx="56" cy="15" r="7" />
      <path className="welcome-dog-coat" d="M51 11l3-7 5 6z" />
      <ellipse className="welcome-dog-white" cx="58.5" cy="12.5" rx="1.3" ry="4.2" transform="rotate(20 58.5 12.5)" />
      <ellipse className="welcome-dog-white" cx="63" cy="18.5" rx="5.2" ry="3.3" />
      <circle className="welcome-dog-coat" cx="68" cy="17.6" r="1.7" />
      <circle className="welcome-dog-white" cx="58.6" cy="14.6" r="1.4" /><circle className="welcome-dog-coat" cx="59" cy="14.6" r=".7" />
    </g>
    <g className="welcome-dog-leg back near"><line x1="26" y1="28" x2="29" y2="40" /><circle cx="29" cy="40" r="2.2" /></g>
    <g className="welcome-dog-leg front near"><line x1="43" y1="28" x2="40" y2="40" /><circle cx="40" cy="40" r="2.2" /></g>
  </svg>;
}

function SheepdogFront() {
  return <svg className="welcome-dog-shape front" viewBox="0 0 72 48">
    <ellipse className="welcome-dog-coat" cx="36" cy="27" rx="10" ry="8" />
    <g className="welcome-dog-paw left"><rect className="welcome-dog-coat" x="29.5" y="29" width="5" height="13" rx="2.5" /><ellipse className="welcome-dog-white" cx="32" cy="42" rx="3" ry="2.2" /></g>
    <g className="welcome-dog-paw right"><rect className="welcome-dog-coat" x="37.5" y="29" width="5" height="13" rx="2.5" /><ellipse className="welcome-dog-white" cx="40" cy="42" rx="3" ry="2.2" /></g>
    <ellipse className="welcome-dog-white" cx="36" cy="27" rx="6" ry="7.5" />
    <path className="welcome-dog-coat" d="M28.5 12l1-10 5 6.5zM43.5 12l-1-10-5 6.5z" />
    <circle className="welcome-dog-coat" cx="36" cy="14.5" r="8.5" />
    <ellipse className="welcome-dog-white" cx="36" cy="13" rx="1.6" ry="6" />
    <ellipse className="welcome-dog-white" cx="36" cy="19.5" rx="4.6" ry="3.6" />
    <ellipse className="welcome-dog-coat" cx="36" cy="18" rx="1.9" ry="1.3" />
    <circle className="welcome-dog-white" cx="32.3" cy="13.3" r="1.4" /><circle className="welcome-dog-coat" cx="32.3" cy="13.5" r=".75" />
    <circle className="welcome-dog-white" cx="39.7" cy="13.3" r="1.4" /><circle className="welcome-dog-coat" cx="39.7" cy="13.5" r=".75" />
  </svg>;
}

function SheepdogBack() {
  return <svg className="welcome-dog-shape back" viewBox="0 0 72 48">
    <path className="welcome-dog-coat" d="M29.5 9l1-8 4.5 5.5zM42.5 9l-1-8-4.5 5.5z" />
    <circle className="welcome-dog-coat" cx="36" cy="11" r="7" />
    <ellipse className="welcome-dog-white" cx="36" cy="18" rx="8" ry="3" />
    <g className="welcome-dog-paw left"><rect className="welcome-dog-coat" x="28.5" y="30" width="5.5" height="12" rx="2.7" /><ellipse className="welcome-dog-white" cx="31.2" cy="42" rx="3" ry="2.2" /></g>
    <g className="welcome-dog-paw right"><rect className="welcome-dog-coat" x="38" y="30" width="5.5" height="12" rx="2.7" /><ellipse className="welcome-dog-white" cx="40.8" cy="42" rx="3" ry="2.2" /></g>
    <ellipse className="welcome-dog-coat" cx="36" cy="26" rx="9.5" ry="9" />
    <g className="welcome-dog-tail"><path className="welcome-dog-coat" d="M33.5 26c-1-7 0-13 3.5-19 2 6 1.5 13-1 19z" /><circle className="welcome-dog-white" cx="37" cy="8" r="2.4" /></g>
  </svg>;
}
