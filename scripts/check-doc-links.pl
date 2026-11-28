#!/usr/bin/env perl
# Check relative links and image paths in Markdown/HTML under the repository.
# Usage: perl scripts/check-doc-links.pl
use strict; use warnings;
use File::Spec; use File::Basename; use File::Find;

my $root = File::Spec->canonpath(File::Spec->rel2abs(File::Spec->catdir(dirname(__FILE__), '..')));
my @files;
find({ no_chdir => 1, wanted => sub {
  return unless /\.(md|html)$/;
  return if m{/target/|node_modules|/\.git/};
  push @files, $_;
} }, $root);

my $bad = 0; my $total = 0;
for my $f (sort @files) {
  my $txt = do { local $/; open my $h, '<', $f or next; my $c = <$h>; close $h; $c };
  $txt =~ s/```.*?```//gs;
  $txt =~ s/`[^`\n]*`//g;
  my $dir = dirname($f);
  while ($txt =~ /\!?\[[^\]]*\]\(([^)\s]+)\)/g) {
    my $t = $1; $total++;
    next if $t =~ m{^(?:[a-zA-Z][a-zA-Z0-9+.\-]*:|//|#)};
    (my $p = $t) =~ s/#.*$//;
    next if $p eq '';
    my $abs = File::Spec->canonpath(File::Spec->rel2abs($p, $dir));
    next if -e $abs;
    $bad++;
    (my $rel = $f) =~ s{^\Q$root\E/}{};
    print "BROKEN  $rel  ->  $t\n";
  }
}
print "checked $total links in " . scalar(@files) . " files; broken: $bad\n";
exit($bad ? 1 : 0);
