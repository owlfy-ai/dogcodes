/**
 * The running sheepdog, for main areas only (startup, conversation). Side panels use
 * skeletons and inline status uses LoadingStatus, so only one dog is ever on screen.
 * Shared with the pre-React startup screen; the animation lives inside the SVG.
 */
export function ActivityLoader() {
  return <img className="activity-loader" src="/loading-mark.svg?v=dog-gallop-1" width={96} height={96} alt="" aria-hidden="true" draggable={false} />;
}
