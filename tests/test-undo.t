  $ cat >> $HGRCPATH <<EOF
  > [extensions]
  > undo = $TESTDIR/../hgext3rd/undo.py
  > [undo]
  > _duringundologlock=1
  > EOF

Build up a repo

  $ hg init repo
  $ cd repo

Test data store

  $ hg book master
  $ touch a1 && hg add a1 && hg ci -ma1
  $ touch a2 && hg add a2 && hg ci -ma2
  $ hg book feature1
  $ touch b && hg add b && hg ci -mb
  $ hg up -q master
  $ touch c1 && hg add c1 && hg ci -mc1
  created new head
  $ touch c2 && hg add c2 && hg ci -mc2
  $ hg book feature2
  $ touch d && hg add d && hg ci -md
  $ hg debugindex .hg/undolog/command.i
     rev    offset  length   base linkrev nodeid       p1           p2
       0         0       0      0       1 b80de5d13875 000000000000 000000000000
       1         0      12      1       1 440cdcef588f 000000000000 000000000000
       2        12       8      2       1 86fddc37572c 000000000000 000000000000
       3        20       8      3       1 388d40a434df 000000000000 000000000000
       4        28      14      4       1 1cafbfad488a 000000000000 000000000000
       5        42       7      5       1 8879b3bd818b 000000000000 000000000000
       6        49      13      6       1 b0f66da09921 000000000000 000000000000
       7        62       8      7       1 004b7198dafe 000000000000 000000000000
       8        70       8      8       1 60920018c706 000000000000 000000000000
       9        78      14      9       1 c3e212568400 000000000000 000000000000
      10        92       7     10       1 9d609b5b001c 000000000000 000000000000
  $ hg debugdata .hg/undolog/command.i 0
  $ hg debugdata .hg/undolog/command.i 1
  book\x00master (no-eol) (esc)
  $ hg debugdata .hg/undolog/command.i 2
  ci\x00-ma1 (no-eol) (esc)
  $ hg debugdata .hg/undolog/command.i 3
  ci\x00-ma2 (no-eol) (esc)
  $ hg debugdata .hg/undolog/bookmarks.i 0
  $ hg debugdata .hg/undolog/bookmarks.i 1
  master 0000000000000000000000000000000000000000 (no-eol)
  $ hg debugdata .hg/undolog/bookmarks.i 2
  master df4fd610a3d6ca792281e7576587fa18f940d37a (no-eol)
  $ hg debugdata .hg/undolog/workingparent.i 0
  0000000000000000000000000000000000000000 (no-eol)
  $ hg debugdata .hg/undolog/workingparent.i 1
  df4fd610a3d6ca792281e7576587fa18f940d37a (no-eol)
  $ hg debugdata .hg/undolog/draftheads.i 1
  df4fd610a3d6ca792281e7576587fa18f940d37a (no-eol)
  $ hg debugdata .hg/undolog/draftheads.i 2
  b68836a6e2cac33ba33a20249b85a486eec78186 (no-eol)
  $ hg debugdata .hg/undolog/index.i 1
  bookmarks 8153d44860d076e9c328951c8f36cf8daebe695a
  command 440cdcef588f9a594c5530a5b6dede39a96d930d
  date * (glob)
  draftheads b80de5d138758541c5f05265ad144ab9fa86d1db
  workingparent fcb754f6a51eaf982f66d0637b39f3d2e6b520d5 (no-eol)
  $ touch a3 && hg add a3
  $ hg commit --amend
  $ hg debugdata .hg/undolog/command.i 11
  commit\x00--amend (no-eol) (esc)

Test debugundohistory
  $ hg debugundohistory -l
  0: commit --amend
  1: ci -md
  2: book feature2
  3: ci -mc2
  4: ci -mc1
  $ hg update master
  0 files updated, 0 files merged, 2 files removed, 0 files unresolved
  (activating bookmark master)
  $ echo "test" >> a1
  $ hg commit -m "words"
  created new head
  $ hg debugundohistory -l
  0: commit -m words
  1: update master
  2: commit --amend
  3: ci -md
  4: book feature2
  $ hg debugundohistory -n 0
  command:
  	commit -m words
  bookmarks:
  	feature1 49cdb4091aca3c09f402ff001cd20cf086873683
  	feature2 296fda51a303650465d07a1cd054075cbe6d3cbd
  	master 0a3dd3e15e65b90836f492112d816f3ee073d897
  date:
  	* (glob)
  draftheads:
  	ADDED:
  		0a3dd3e15e65b90836f492112d816f3ee073d897
  	REMOVED:
  	
  workingparent:
  	0a3dd3e15e65b90836f492112d816f3ee073d897

Test gap in data (extension dis and enabled)
  $ hg debugundohistory -l
  0: commit -m words
  1: update master
  2: commit --amend
  3: ci -md
  4: book feature2
  $ cat >> $HGRCPATH <<EOF
  > [extensions]
  > undo =!
  > EOF
  $ touch cmiss && hg add cmiss && hg ci -mcmiss
  $ cat >>$HGRCPATH <<EOF
  > [extensions]
  > undo = $TESTDIR/../hgext3rd/undo.py
  > EOF
  $ touch a5 && hg add a5 && hg ci -ma5
  $ hg debugundohistory -l
  0: ci -ma5
  1:  -- gap in log -- 
  2: commit -m words
  3: update master
  4: commit --amend
  $ hg debugundohistory 1
  command:
  	unkown command(s) run, gap in log
  bookmarks:
  	feature1 49cdb4091aca3c09f402ff001cd20cf086873683
  	feature2 296fda51a303650465d07a1cd054075cbe6d3cbd
  	master 1dafc0b436123cab96f82a8e9e8d1d42c0301aaa
  date:
  	* (glob)
  draftheads:
  	ADDED:
  		1dafc0b436123cab96f82a8e9e8d1d42c0301aaa
  	REMOVED:
  		0a3dd3e15e65b90836f492112d816f3ee073d897
  workingparent:
  	1dafc0b436123cab96f82a8e9e8d1d42c0301aaa

Index out of bound error
  $ hg debugundohistory -n 50
  abort: index out of bounds
  [255]

Revset tests
  $ hg log -G -r 'draft()' --hidden > /dev/null
  $ hg debugundohistory -n 0
  command:
  	ci -ma5
  bookmarks:
  	feature1 49cdb4091aca3c09f402ff001cd20cf086873683
  	feature2 296fda51a303650465d07a1cd054075cbe6d3cbd
  	master aa430c8afedf9b2ec3f0655d39eef6b6b0a2ddb6
  date:
  	* (glob)
  draftheads:
  	ADDED:
  		aa430c8afedf9b2ec3f0655d39eef6b6b0a2ddb6
  	REMOVED:
  		1dafc0b436123cab96f82a8e9e8d1d42c0301aaa
  workingparent:
  	aa430c8afedf9b2ec3f0655d39eef6b6b0a2ddb6

Test 'olddraft([NUM])' revset
  $ hg log -G -r 'olddraft(0) - olddraft(1)' --hidden -T compact
  @  8[tip][master]   aa430c8afedf   1970-01-01 00:00 +0000   test
  |    a5
  ~

Test undolog lock
  $ hg log --config hooks.duringundologlock="sleep 1" > /dev/null &
  $ sleep 0.1
  $ hg st --time
  time: real [1-9]*\..* (re)
