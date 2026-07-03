-- Tone of Voice / Writing Style profiles.
-- Each row is a saved writing style a Pro/Max user can apply to a skill (or set
-- as their global default). Unlike profiles, these rows are fully user-owned and
-- edited client-side, so RLS is scoped tightly to the owner via auth.uid().
--
-- Note: profiles RLS already blocks users from changing their own subscription —
-- there is NO user UPDATE policy on public.profiles, so self-service upgrades are
-- impossible. This table's user-writable policies are safe precisely because the
-- subscription gate lives elsewhere (Edge Functions + profiles read-only RLS).

create table if not exists public.tone_of_voice (
  id         uuid primary key default gen_random_uuid(),
  -- Defaults to the caller's uid so PostgREST inserts (which send only
  -- name/content) auto-stamp the owner and satisfy the insert RLS check.
  user_id    uuid not null default auth.uid() references auth.users on delete cascade,
  name       text not null,
  content    text not null default '',
  is_default boolean not null default false,
  created_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create index if not exists tone_of_voice_user_id_idx
  on public.tone_of_voice (user_id);

alter table public.tone_of_voice enable row level security;

create policy "Users can read own tones"
  on public.tone_of_voice for select
  using (auth.uid() = user_id);

create policy "Users can insert own tones"
  on public.tone_of_voice for insert
  with check (auth.uid() = user_id);

create policy "Users can update own tones"
  on public.tone_of_voice for update
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

create policy "Users can delete own tones"
  on public.tone_of_voice for delete
  using (auth.uid() = user_id);

-- Marks a single tone as the caller's global default, clearing the flag on all
-- their other tones in one atomic call. Security definer so the two-statement
-- flip runs as a unit; the auth.uid() = user_id guards keep it owner-scoped.
create or replace function public.set_default_tone(p_id uuid)
returns void
language plpgsql
security definer
set search_path = public
as $$
begin
  update public.tone_of_voice
    set is_default = false, updated_at = now()
    where user_id = auth.uid() and is_default = true;

  update public.tone_of_voice
    set is_default = true, updated_at = now()
    where id = p_id and user_id = auth.uid();
end;
$$;

grant execute on function public.set_default_tone(uuid) to authenticated;
