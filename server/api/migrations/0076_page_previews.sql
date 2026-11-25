-- What a share link previews as, when its host sent it with the share: the page's title and the
-- sharer's name, the only two things of a page the server can read. Both go when the share ends.
alter table pages
  add column title text check (char_length(title) between 1 and 200),
  add column shared_by text check (char_length(shared_by) between 1 and 80);
