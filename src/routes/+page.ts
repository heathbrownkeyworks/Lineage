import { redirect } from "@sveltejs/kit";

// The app opens on Find Assets.
export function load(): never {
  redirect(307, "/find");
}
