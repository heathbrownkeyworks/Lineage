/**
 * Clean Preset categories, shared by the single-file and batch screens. The
 * defaults were chosen with Heath: everything an author's own setup leaks,
 * but not head & neck, which is usually part of the character's look. Face
 * overlays aren't a category at all — nothing here can strip a face.
 */
import type { CleanCategory } from "./types";

export type CategoryInfo = {
  id: CleanCategory;
  label: string;
  hint: string;
  defaultOn: boolean;
};

export const CATEGORIES: CategoryInfo[] = [
  {
    id: "body_morphs",
    label: "Body morphs",
    hint: "BodySlide sliders and XPMSE morphs",
    defaultOn: true,
  },
  {
    id: "body_overlays",
    label: "Body overlays",
    hint: "Tattoos and paint on body, hands and feet",
    defaultOn: true,
  },
  {
    id: "skeleton",
    label: "Height & skeleton",
    hint: "Root scale (height), spine, limbs, fingers",
    defaultOn: true,
  },
  {
    id: "weapon_camera",
    label: "Weapon & camera",
    hint: "Weapon, shield and quiver placement; camera nodes",
    defaultOn: true,
  },
  {
    id: "head_neck",
    label: "Head & neck",
    hint: "Head scale and neck — usually part of the look",
    defaultOn: false,
  },
];

export const DEFAULT_CATEGORIES: CleanCategory[] = CATEGORIES.filter((c) => c.defaultOn).map(
  (c) => c.id,
);

export function categoryLabel(id: CleanCategory): string {
  return CATEGORIES.find((c) => c.id === id)?.label ?? id;
}

/** "12 body morphs · 3 body overlays" — lowercased labels with counts. */
export function describeCounts(
  counts: Partial<Record<CleanCategory, number>>,
  only?: CleanCategory[],
): string {
  return CATEGORIES.filter((c) => (counts[c.id] ?? 0) > 0 && (!only || only.includes(c.id)))
    .map((c) => `${counts[c.id]} ${c.label.toLowerCase()}`)
    .join(" · ");
}
