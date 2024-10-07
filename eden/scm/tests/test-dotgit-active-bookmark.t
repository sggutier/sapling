#require git no-windows

Test active bookmark sync between Git and Sl (dotgit).

  $ . $TESTDIR/git.sh

Create commit via git, forbidden branch name:

  $ git init -qb main client-repo1
  $ cd client-repo1

  $ git commit -qm A --allow-empty

  $ sl bookmarks
  no bookmarks set
  $ git branch
  * main

Create commit via sl, forbidden branch name:

  $ cd
  $ git init -qb main client-repo2
  $ cd client-repo2

  $ sl commit -qm A --config ui.allowemptycommit=true

  $ sl bookmarks
  no bookmarks set
  $ git branch
  * main

Create commit via git, allowed branch name:

  $ git init -qb foo client-repo3
  $ cd client-repo3

  $ git commit -qm A --allow-empty
FIXME: "foo" should be active in sl.
  $ sl bookmarks
     foo                       04d80a1d9a8a
  $ git branch
  * foo

Create commit via sl, allowed branch name:

  $ cd
  $ git init -qb foo client-repo3
  $ cd client-repo3

  $ sl commit -qm A --config ui.allowemptycommit=true
FIXME: "foo" should be active in sl.
  $ sl bookmarks
     foo                       b2698e270d93
  $ git branch
  * foo
