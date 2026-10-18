create table if not exists accounts (
  id integer primary key,
  email text not null unique,
  key text not null unique
);

create table if not exists bookmarks (
  id integer primary key,
  account_id integer not null references accounts (id),
  url text not null,
  title text not null,
  created_at text not null
);
