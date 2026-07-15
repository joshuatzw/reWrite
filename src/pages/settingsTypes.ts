export interface AuthState {
  logged_in: boolean;
  email: string;
  name: string;
  is_subscribed: boolean;
  subscription_valid_until: string | null;
  rewrite_count: number;
  plan: "pro" | "max" | null;
}

export type ActiveView = "home" | "history" | "skills" | "settings" | "accessibility";
