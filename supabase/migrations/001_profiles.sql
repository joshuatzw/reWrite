-- profiles table: Stripe cache + free-tier usage tracking
-- Subscription truth lives in Stripe; this is a time-bounded read cache.

create table if not exists public.profiles (
  id                       uuid primary key references auth.users on delete cascade,
  stripe_customer_id       text unique,

  -- Written only by sync-subscription and stripe-webhook
  is_subscribed            boolean not null default false,
  subscription_valid_until  timestamptz,
  last_synced_at           timestamptz,

  -- Free-tier usage (Stripe doesn't track per-call usage natively)
  rewrite_count            int not null default 0,
  rewrite_month            text   -- 'YYYY-MM', compared on each rewrite call
);

alter table public.profiles enable row level security;

-- Users can read their own profile (for frontend display)
create policy "Users can read own profile"
  on public.profiles for select
  using (auth.uid() = id);

-- Edge Functions use the service role key and bypass RLS for writes.

-- Auto-create profile row on auth.users insert
create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer set search_path = public
as $$
begin
  insert into public.profiles (id)
  values (new.id)
  on conflict (id) do nothing;
  return new;
end;
$$;

drop trigger if exists on_auth_user_created on auth.users;

create trigger on_auth_user_created
  after insert on auth.users
  for each row execute procedure public.handle_new_user();
