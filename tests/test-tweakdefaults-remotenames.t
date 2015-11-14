Check for remotenames and skip if not present
  $ $PYTHON -c 'import remotenames' || exit 80

Set up
  $ extpath=$(dirname $TESTDIR)
  $ cp $extpath/tweakdefaults.py $TESTTMP # use $TESTTMP substitution in message
  $ cat >> $HGRCPATH << EOF
  > [extensions]
  > rebase=
  > remotenames=
  > tweakdefaults=$TESTTMP/tweakdefaults.py
  > EOF

  $ hg init repo
  $ echo a > repo/a
  $ hg -R repo commit -qAm aa
  $ hg -R repo bookmark one -i
  $ echo b > repo/b
  $ hg -R repo commit -qAm bb
  $ hg -R repo bookmark two -i
  $ echo c > repo/c
  $ hg -R repo commit -qAm cc
  $ hg -R repo bookmark three -i
  $ hg clone -q repo clone
  $ cd clone

Test that hg pull --rebase aborts without --dest
  $ hg log -G --all -T '{node|short} {bookmarks} {remotenames}'
  @  083f922fc4a9  default/three default/default
  |
  o  301d76bdc3ae  default/two
  |
  o  8f0162e483d0  default/one
  
  $ hg up -q default/one
  $ touch foo
  $ hg commit -qAm 'foo'
  $ hg pull --rebase
  abort: you must use a bookmark with tracking or manually specify a destination for the rebase
  (set up tracking with `hg book <name> -t <destination>` or manually supply --dest / -d)
  [255]
  $ hg bookmark bm
  $ hg pull --rebase
  abort: you must use a bookmark with tracking or manually specify a destination for the rebase
  (set up tracking with `hg book -t <destination>` or manually supply --dest / -d)
  [255]
  $ hg book bm -t default/two
  $ hg pull --rebase
  pulling from $TESTTMP/repo
  searching for changes
  no changes found
  rebasing 3:3de6bbccf693 "foo" (tip bm)
  saved backup bundle to $TESTTMP/clone/.hg/strip-backup/3de6bbccf693-0dce0663-backup.hg (glob)
  $ hg pull --rebase --dest three
  pulling from $TESTTMP/repo
  searching for changes
  no changes found
  rebasing 3:54ac787ff1c5 "foo" (tip bm)
  saved backup bundle to $TESTTMP/clone/.hg/strip-backup/54ac787ff1c5-4c2ca3a1-backup.hg (glob)

Test that hg pull --update aborts without --dest
  $ hg pull --update
  abort: you must specify a destination for the update
  (use `hg pull --update --dest <destination>`)
  [255]
  $ hg pull --update --dest one
  pulling from $TESTTMP/repo
  searching for changes
  no changes found
  0 files updated, 0 files merged, 3 files removed, 0 files unresolved
  (leaving bookmark bm)

