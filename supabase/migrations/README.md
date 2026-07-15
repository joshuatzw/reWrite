# Migration conventions

Rules for writing migrations in this project. These exist because we shipped
bugs by not following them — read before adding a migration.

## SECURITY DEFINER functions must be locked down explicitly

A `SECURITY DEFINER` function runs with the **definer's** privileges and
**bypasses RLS**. Postgres grants `EXECUTE` to `PUBLIC` **by default**, so an
explicit `grant execute ... to service_role` does **not** lock the function
down — `anon` and `authenticated` can still call it directly over PostgREST
(`/rest/v1/rpc/<fn>`).

**Rule:** every `SECURITY DEFINER` function must revoke the default PUBLIC grant.

```sql
create or replace function public.my_fn(...) returns ...
  language plpgsql security definer set search_path = public
as $$ ... $$;

-- Required for every SECURITY DEFINER function:
revoke execute on function public.my_fn(...) from public, anon, authenticated;
grant  execute on function public.my_fn(...) to service_role;  -- if the edge fn calls it
```

Verify with the audit query (all three functions here should read
`{postgres,service_role}` — never `-`/PUBLIC, `anon`, or `authenticated`):

```sql
select p.proname, p.prosecdef as security_definer,
       pg_get_function_identity_arguments(p.oid) as args,
       array_agg(distinct acl.grantee::regrole) as granted_to
from pg_proc p
join pg_namespace n on n.oid = p.pronamespace
left join lateral aclexplode(p.proacl) acl on true
where n.nspname = 'public'
group by p.proname, p.prosecdef, p.oid
order by p.proname;
```

Real bug this caused: `check_and_increment_usage` was PUBLIC-executable, letting
a signed-in user call it with a bogus `p_month` to reset their own usage counter
and get unlimited free rewrites on our Anthropic bill. Fixed in
`005_lock_down_definer_functions.sql`.

> Trigger and event-trigger functions (`handle_new_user`, `rls_auto_enable`) can
> also be revoked from PUBLIC safely — the trigger mechanism fires them without
> checking `EXECUTE`.

## Every table must have RLS enabled

Client-facing tables in `public` must have RLS **enabled** with owner-scoped
policies. A policy with RLS *disabled* is ignored — the table is fully open.

```sql
alter table public.my_table enable row level security;

create policy "Users manage own rows" on public.my_table
  for all using (auth.uid() = user_id) with check (auth.uid() = user_id);
```

Writes that must not be client-controllable (billing flags, usage counters) get
**no** write policy — mutate them only from edge functions using the
`service_role` key, which bypasses RLS.

Audit query — every row must be `rls_enabled = true`:

```sql
select c.relname as table_name, c.relrowsecurity as rls_enabled,
       coalesce(p.cnt,0) as policy_count
from pg_class c
join pg_namespace n on n.oid = c.relnamespace
left join (select tablename, count(*) cnt from pg_policies
           where schemaname='public' group by 1) p on p.tablename = c.relname
where n.nspname='public' and c.relkind='r'
order by c.relrowsecurity asc, c.relname;
```

## Capture dashboard changes as migrations

Anything created in the Supabase dashboard (tables, RPCs, policies) is **not**
version-controlled and drifts from the repo. The deprecated `tone_of_voice`
table lived only in the dashboard and lingered after the app-side feature was
removed (see `004_drop_tone_of_voice.sql`). Always add a migration for schema
changes instead of editing live.
