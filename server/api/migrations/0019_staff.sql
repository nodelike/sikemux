-- Someone who makes Sikemux: their hosts take each build of main before nightly does.
alter table users add column staff boolean not null default false;
