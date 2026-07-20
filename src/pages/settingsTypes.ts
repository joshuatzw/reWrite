export type Plan = "pro" | "max";

export interface AuthState {
  logged_in: boolean;
  email: string;
  name: string;
  is_subscribed: boolean;
  subscription_valid_until: string | null;
  rewrite_count: number;
  plan: Plan | null;
}

/// An "Upgrade" click handed over from the website's pricing page via the
/// `rewrite://upgrade` deep link. `plan` is null when the link named no valid
/// plan, in which case the dialog opens on the chooser with nothing
/// preselected. Treat it as a UI hint only — the authoritative plan is
/// whatever `AuthState` reports after the server confirms payment.
export interface UpgradeRequest {
  plan: Plan | null;
}

export type ActiveView = "home" | "history" | "skills" | "settings" | "accessibility";
