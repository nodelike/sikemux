-- When the Mac sign-in behind the host that approved a host's code was made, so the host it adds
-- carries it too and goes when the strip ends that sign-in. Null when a session approved.
alter table host_sign_ins add column approved_mac_signed_in_at timestamptz;
