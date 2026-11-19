alter table events
  drop constraint events_type_check,
  add constraint events_type_check check (
    type in (
      'device.added', 'device.changed', 'device.revoked', 'account.deleted',
      'stash.added', 'stash.changed', 'stash.removed', 'stash.seen',
      'vault.changed', 'mods.changed', 'plan.changed', 'orgs.changed'
    )
  );
