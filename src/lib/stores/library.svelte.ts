/**
 * Module-level home for the Collection Review report, so navigating away
 * from /library and back within the same session doesn't lose a report
 * that's already been run (the review scans the whole collection — a few
 * seconds of work worth not repeating on every visit).
 */
import type { CollectionReport } from "$lib/types";

export const libraryReview = $state<{ report: CollectionReport | null }>({ report: null });
