export interface AuthState {
  logged_in: boolean;
  email: string;
  is_subscribed: boolean;
  subscription_valid_until: string | null;
  rewrite_count: number;
}

export type ActiveView = "home" | "history" | "skills" | "settings";
