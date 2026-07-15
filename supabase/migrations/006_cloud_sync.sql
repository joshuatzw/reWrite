-- Per-user skills/config blob. Rewritten prose is never stored here.
create table public.user_skills (
  user_id     uuid primary key references auth.users on delete cascade,
  config      jsonb not null,
  updated_at  timestamptz not null default now()
);

alter table public.user_skills enable row level security;

create policy "own skills"
  on public.user_skills
  for all
  using (auth.uid() = user_id)
  with check (auth.uid() = user_id);

-- Network requests can arrive out of order. Keep the row with the greatest
-- logical edit timestamp so a delayed older upsert cannot undo a newer edit.
create or replace function public.keep_newest_user_skills()
returns trigger
language plpgsql
set search_path = public
as $$
begin
  if new.updated_at < old.updated_at then
    return old;
  end if;
  return new;
end;
$$;

create trigger keep_newest_user_skills_before_update
  before update on public.user_skills
  for each row execute function public.keep_newest_user_skills();

-- Append-only history metadata. Deliberately no input/output text columns.
create table public.rewrite_history (
  id                text primary key,
  user_id           uuid not null references auth.users on delete cascade,
  timestamp_ms      bigint not null,
  skill_id          text not null,
  skill_name        text not null,
  output_word_count int not null default 0,
  created_at        timestamptz not null default now()
);

alter table public.rewrite_history enable row level security;

create policy "own history read"
  on public.rewrite_history
  for select
  using (auth.uid() = user_id);

create policy "own history insert"
  on public.rewrite_history
  for insert
  with check (auth.uid() = user_id);

create index rewrite_history_user_ts
  on public.rewrite_history (user_id, timestamp_ms desc);

revoke execute on function public.keep_newest_user_skills()
  from public, anon, authenticated;
