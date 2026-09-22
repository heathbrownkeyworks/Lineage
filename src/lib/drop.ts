// Files dropped on the window: what the drop will do on the page it lands on.

export const isPreset = (path: string) => path.toLowerCase().endsWith(".jslot");

const nameOf = (path: string) => path.split(/[\\/]/).pop() ?? path;

export type DropPlan = {
  /** Whether dropping does anything. */
  usable: boolean;
  /** What the overlay says while dragging. */
  message: string;
  /** The route that handles it. */
  route: string;
};

/** Pages that act on a drop themselves; anything else hands it to Find Assets. */
const HANDLERS = ["/find", "/remove", "/compare", "/release"];

export function planDrop(pathname: string, paths: string[]): DropPlan {
  const presets = paths.filter(isPreset);
  const page = HANDLERS.find((h) => pathname === h || pathname.startsWith(`${h}/`)) ?? "/find";
  if (page === "/release") {
    // Folders can't be told from files here; Release lists what's inside.
    const others = paths.length - presets.length;
    return {
      usable: paths.length > 0,
      message:
        others === 0
          ? `Drop to add ${presets.length === 1 ? "1 preset" : `${presets.length} presets`} to the pack`
          : "Drop to add these presets and folders to the pack",
      route: page,
    };
  }
  if (presets.length === 0) {
    return { usable: false, message: "Only .jslot presets can be dropped here.", route: page };
  }
  const first = nameOf(presets[0]);
  switch (page) {
    case "/remove":
      return { usable: true, message: `Drop to open ${first} in Clean Preset`, route: page };
    case "/compare":
      return {
        usable: true,
        message:
          presets.length > 1 ? `Drop to compare ${first} with ${nameOf(presets[1])}` : `Drop to compare ${first}`,
        route: page,
      };
    default:
      return { usable: true, message: `Drop to trace ${first} in Find Assets`, route: "/find" };
  }
}
