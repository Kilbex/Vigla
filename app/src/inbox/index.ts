// S3 — barrel re-exports for the inbox module.
export {
  type EventVisibility,
  type InboxAction,
  type InboxCard,
  type InboxKind,
  type InboxState,
  type Severity,
  boundLabel,
  emptyInboxState,
  kindLabel,
  requiresAction,
  severityGlyph,
} from "./types";
export { applyInboxAction, selectInbox } from "./InboxState";
export { default as InboxCardView } from "./InboxCard";
export { default as EscalationFeed } from "./EscalationFeed";
export { default as InboxOverview } from "./InboxOverview";
export { fetchVisibility } from "./visibility-client";
