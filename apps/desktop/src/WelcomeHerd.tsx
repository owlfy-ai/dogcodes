import { useLayoutEffect, useRef, type CSSProperties, type ReactNode } from "react";

export type HerdTone = "installed" | "scanning" | "failed" | "missing";

// A meadow seen from the side. Positions are fractions of the herd width from the origin;
// y grows toward the viewer, so nearer animals are drawn larger and in front.
const rowGap = .065;
const outrunDepth = .27;
const timing = { eye: 1.1, drive: 5.4, settle: 1.6, outrun: 2.8 };
type Phase = keyof typeof timing;
const phases: Phase[] = ["eye", "drive", "settle", "outrun"];
// Undetected agents graze far off on the back of the meadow.
const strayX = [-.44, -.36, -.28, -.2, -.12, -.04, .04, .12, .2, .28, .36, .44];
const strayOrder = [0, 11, 5, 6, 2, 9, 3, 8, 1, 10, 4, 7];

const ease = (value: number) => (1 - Math.cos(Math.PI * Math.min(1, Math.max(0, value)))) / 2;
const depthScale = (y: number) => 1 + y * .6;

/** Staggered rows, so every sheep's icon stays clear of the sheep in front of it. */
function flockLayout(count: number) {
  const size = count <= 4 ? .13 : count <= 6 ? .12 : count <= 9 ? .105 : .095;
  const rows = count <= 3 ? 1 : count <= 6 ? 2 : count <= 9 ? 3 : 4;
  const offsets: [number, number][] = [];
  for (let row = 0; row < rows; row++) {
    const inRow = Math.floor(count / rows) + (row < count % rows ? 1 : 0);
    const shift = rows > 1 ? (row % 2 ? .25 : -.25) * size : 0;
    for (let col = 0; col < inRow; col++) {
      const index = offsets.length;
      offsets.push([(col - (inRow - 1) / 2) * size * 1.04 + shift + ((index * 37) % 7 - 3) * .003,
        (row - (rows - 1) / 2) * rowGap + ((index * 53) % 5 - 2) * .003]);
    }
  }
  const half = Math.max(0, ...offsets.map(([x]) => Math.abs(x))) + size / 2;
  return { offsets, size, half };
}

/** The welcome hero: a sheepdog drives the detected agents across the meadow and back. */
export function WelcomeHerd({ agents, renderIcon }: { agents: { id: string; tone: HerdTone }[]; renderIcon: (id: string) => ReactNode }) {
  const herd = useRef<HTMLDivElement>(null);
  const dogRef = useRef<HTMLDivElement>(null);
  const flockIds = agents.filter(a => a.tone === "installed").map(a => a.id);
  const strayIds = agents.filter(a => a.tone === "failed" || a.tone === "missing").map(a => a.id);
  const flock = flockLayout(flockIds.length);

  useLayoutEffect(() => {
    const root = herd.current, dog = dogRef.current;
    if (!root || !dog) return;
    const still = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const offsets = new WeakMap<HTMLElement, [number, number]>();
    const facing = new WeakMap<HTMLElement, number>();
    let direction = 1, phase: Phase = "eye", phaseStart = performance.now(), last = phaseStart;
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
      let t = (now - phaseStart) / 1000;
      if (!still && t > timing[phase]) {
        if (phase === "outrun") direction = -direction;
        phase = phases[(phases.indexOf(phase) + 1) % phases.length];
        phaseStart = now;
        t = 0;
      }
      const half = Number(root.dataset.half) || .1;
      const reach = half + .08;
      // Drive as far across the meadow as the flock's width allows.
      const restX = Math.min(.26, Math.max(.12, .44 - reach));
      const from = -direction * restX, to = direction * restX;
      const driveSpan = timing.drive - .9;
      const outrun = phase === "outrun" ? ease(t / timing.outrun) : 0;

      root.querySelectorAll<HTMLElement>(".welcome-sheep.penned").forEach(sheep => {
        // Ease toward a new slot when the flock grows, instead of jumping.
        const target = [Number(sheep.dataset.ox), Number(sheep.dataset.oy)];
        const offset = offsets.get(sheep) ?? [target[0], target[1]];
        offset[0] += (target[0] - offset[0]) * Math.min(1, elapsed * 3);
        offset[1] += (target[1] - offset[1]) * Math.min(1, elapsed * 3);
        offsets.set(sheep, offset);
        // 0 at the edge nearest the driving dog, 1 at the leading edge: the back of the flock moves first.
        const lead = Math.min(1, Math.max(0, (direction * offset[0] + half) / (2 * half)));
        const progress = phase === "eye" ? 0 : phase === "drive" ? ease((t - lead * .7) / driveSpan) : 1;
        place(sheep, from + (to - from) * progress + offset[0], offset[1]);
        mark(sheep, "gait", phase === "drive" && progress > 0 && progress < 1 ? "walk" : "idle");
        // Once the dog comes round to the far flank, sheep turn away from it, nearest first.
        const face = phase === "outrun" && outrun > .68 + .25 * (1 - lead) ? -direction : direction;
        if (facing.get(sheep) !== face) {
          if (facing.has(sheep)) (sheep.firstElementChild as HTMLElement | null)?.animate?.([{ translate: "0 0" }, { translate: "0 -14%" }, { translate: "0 0" }], { duration: 300, easing: "ease-out" });
          facing.set(sheep, face);
          sheep.style.setProperty("--facing", `${face}`);
        }
      });

      let x: number, y = 0, gait: string, view = "side";
      if (phase === "eye") { x = from - direction * reach; gait = "eye"; }
      else if (phase === "drive") {
        const progress = ease(t / driveSpan);
        x = from + (to - from) * progress - direction * reach;
        // Wearing: the dog swings side to side behind the flock to keep it moving straight.
        y = Math.sin(t * Math.PI * 2 / 2.4) * .03 * Math.sin(Math.PI * progress);
        gait = progress < 1 ? "drive" : "eye";
      } else if (phase === "settle") { x = to - direction * reach; gait = "down"; }
      else {
        // The outrun loops behind the flock: away from the viewer, across, then back toward them.
        x = to - direction * reach * Math.cos(Math.PI * outrun);
        y = -outrunDepth * Math.sin(Math.PI * outrun);
        const across = Math.abs(reach * Math.sin(Math.PI * outrun)), toward = -outrunDepth * Math.cos(Math.PI * outrun);
        view = Math.abs(toward) > 1.3 * across ? (toward < 0 ? "back" : "front") : "side";
        gait = t / timing.outrun > .97 ? "stand" : "run";
      }
      place(dog, x, y);
      dog.style.setProperty("--face", `${direction}`);
      mark(dog, "view", view);
      mark(dog, "gait", gait);
    };

    tick(performance.now());
    let frame = requestAnimationFrame(function step(now) { tick(now); frame = requestAnimationFrame(step); });
    return () => cancelAnimationFrame(frame);
  }, []);

  return <div className="welcome-herd" ref={herd} data-half={flock.half} aria-hidden="true">
    <div className="welcome-herd-meadow" />
    {agents.map((agent, index) => {
      const inFlock = flockIds.indexOf(agent.id), stray = strayIds.indexOf(agent.id);
      const slot = strayOrder[stray % strayOrder.length];
      const strayY = slot % 2 ? -.3 : -.35;
      const style: Record<string, number> = inFlock >= 0 ? { "--size": flock.size }
        : stray >= 0 ? { "--x": strayX[slot], "--y": strayY, "--size": .1, "--depth": depthScale(strayY) * .75, "--facing": slot < 6 ? 1 : -1, zIndex: 0 }
        : { "--size": flock.size };
      return <span key={agent.id} style={{ ...style, "--graze": `${(index * -1.37) % 5}s` } as CSSProperties}
        data-ox={inFlock >= 0 ? flock.offsets[inFlock][0] : undefined} data-oy={inFlock >= 0 ? flock.offsets[inFlock][1] : undefined}
        className={`welcome-sheep ${agent.tone}${inFlock >= 0 ? " penned" : stray >= 0 ? "" : " away"}`}>
        <span className="welcome-sheep-body">
          <SheepShape />
          <span className="welcome-sheep-tag">{renderIcon(agent.id)}</span>
        </span>
      </span>;
    })}
    <div className="welcome-herd-dog" ref={dogRef}>
      <SheepdogSide /><SheepdogFront /><SheepdogBack />
    </div>
  </div>;
}

function SheepShape() {
  return <svg className="welcome-sheep-shape" viewBox="0 0 60 48">
    <g className="welcome-sheep-legs"><rect x="15" y="32" width="4" height="12" rx="2" /><rect x="23" y="33" width="4" height="12" rx="2" /><rect x="35" y="33" width="4" height="12" rx="2" /><rect x="43" y="32" width="4" height="12" rx="2" /></g>
    <g className="welcome-sheep-wool">
      <circle cx="16" cy="21" r="9" /><circle cx="26" cy="14" r="10" /><circle cx="38" cy="15" r="9.5" /><circle cx="45" cy="25" r="8" />
      <circle cx="36" cy="32" r="9" /><circle cx="23" cy="32" r="9" /><circle cx="12" cy="29" r="7" /><ellipse cx="30" cy="24" rx="17" ry="12" />
    </g>
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
