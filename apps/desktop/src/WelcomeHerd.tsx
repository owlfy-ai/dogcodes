import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";

export type HerdTone = "installed" | "scanning" | "failed" | "missing";

// A meadow seen from the side. Positions are fractions of the herd width from the
// origin; y grows toward the viewer, so nearer animals are drawn larger and in front.
const depthScale = (y: number) => 1 + y * .6;
const alertMillis = 1400;

/** Every supported agent keeps one slot from the start, so nothing reflows or jumps.
 * The flock walks right of centre; the dog drives it from behind, on the left. */
export function herdLayout(count: number) {
  const columns = Math.max(1, Math.min(5, Math.ceil(count / 3)));
  const rows = Math.max(1, Math.ceil(count / columns));
  const size = count <= 6 ? .12 : .105;
  const spacing = size * 1.12, rowGap = size * .56, shiftX = .07, shiftY = -.02;
  const slots: [number, number][] = [];
  for (let row = 0; row < rows; row++) {
    const inRow = Math.min(columns, count - slots.length);
    // Staggered rows keep each sheep's tag clear of the sheep in front of it.
    const stagger = rows > 1 ? (row % 2 ? .25 : -.25) * size : 0;
    for (let col = 0; col < inRow; col++) {
      slots.push([(col - (inRow - 1) / 2) * spacing + stagger + shiftX, (row - (rows - 1) / 2) * rowGap + shiftY]);
    }
  }
  const left = Math.min(...slots.map(([x]) => x)) - size / 2;
  const middle = (Math.min(...slots.map(([, y]) => y)) + Math.max(...slots.map(([, y]) => y))) / 2;
  const dog: [number, number] = [left - .115, middle + .01];
  return { slots, size, dog };
}

/** The welcome hero: the dog drives the flock steadily to the right across a
 * scrolling meadow. Every agent walks in it from the start as an unknown sheep and
 * lights up with its logo once found; the dog looks up when that happens. */
export function WelcomeHerd({ agents, renderIcon }: { agents: { id: string; tone: HerdTone }[]; renderIcon: (id: string) => ReactNode }) {
  const layout = herdLayout(agents.length);
  // Agents already found at first render are not announced again.
  const ready = useRef<Set<string> | null>(null);
  if (ready.current === null) ready.current = new Set(agents.filter(a => a.tone === "installed").map(a => a.id));
  const [alert, setAlert] = useState(false);
  const readyKey = agents.filter(a => a.tone === "installed").map(a => a.id).join();

  useEffect(() => {
    const known = ready.current!;
    const found = readyKey ? readyKey.split(",") : [];
    const fresh = found.some(id => !known.has(id));
    // A rescan clears what was found, so finding it again is announced again.
    ready.current = new Set(found);
    if (!fresh) return;
    setAlert(true);
    const timer = window.setTimeout(() => setAlert(false), alertMillis);
    return () => window.clearTimeout(timer);
  }, [readyKey]);

  const [dogX, dogY] = layout.dog;
  const dogStyle = { "--x": dogX, "--y": dogY, "--depth": depthScale(dogY), zIndex: Math.round((dogY + 1) * 100) } as CSSProperties;

  return <div className="welcome-herd" aria-hidden="true">
    <div className="welcome-herd-meadow">
      <div className="welcome-herd-ground far" />
      <div className="welcome-herd-ground near" />
    </div>
    {agents.map((agent, index) => {
      const [x, y] = layout.slots[index];
      // Uneven steps and drift keep the flock from marching in lockstep.
      const style = {
        "--x": x, "--y": y, "--size": layout.size, "--depth": depthScale(y),
        "--step": `${(.52 + (index % 3) * .05).toFixed(2)}s`, "--step-delay": `-${((index * .37) % 1).toFixed(2)}s`,
        "--drift": `${(3.2 + (index * 1.7) % 2.4).toFixed(1)}s`, "--drift-delay": `-${((index * 1.3) % 3).toFixed(1)}s`,
        zIndex: Math.round((y + 1) * 100),
      } as CSSProperties;
      const lit = agent.tone === "installed";
      return <span key={agent.id} style={style} data-agent={agent.id}
        className={`welcome-sheep ${lit ? "ready" : "unknown"} ${agent.tone}`}>
        <span className="welcome-sheep-body">
          <SheepShape />
          <span className="welcome-sheep-tag">{lit ? renderIcon(agent.id) : <span className="welcome-sheep-unknown">?</span>}</span>
        </span>
      </span>;
    })}
    <div className="welcome-herd-dog" style={dogStyle} data-alert={alert || undefined}>
      <span className="welcome-dog-dust"><i /><i /><i /></span>
      <SheepdogSide />
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
    <DogLeg hip={[24, 27]} className="back far" />
    <DogLeg hip={[45, 28]} className="front far" />
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
    <DogLeg hip={[27, 28]} className="back near" />
    <DogLeg hip={[42, 28]} className="front near" />
  </svg>;
}

/** A leg in two parts: the thigh swings from the hip and the shin folds at the knee,
 * so a stepping paw lifts off the ground instead of sweeping along it. */
function DogLeg({ hip: [x, y], className }: { hip: [number, number]; className: string }) {
  const knee = y + 6.5, paw = 40;
  return <g className={`welcome-dog-leg ${className}`} style={{ transformOrigin: `${x}px ${y}px` }}>
    <line x1={x} y1={y} x2={x} y2={knee} />
    <g className="welcome-dog-shin" style={{ transformOrigin: `${x}px ${knee}px` }}>
      <line x1={x} y1={knee} x2={x} y2={paw} /><circle cx={x} cy={paw} r="2.2" />
    </g>
  </g>;
}
