#!/usr/bin/perl
# Generate Junos $9$ test vectors with Crypt::Juniper, the reference
# implementation of the obfuscation (https://metacpan.org/pod/Crypt::Juniper).
#
#   cpan Crypt::Juniper
#   perl tests/generate_junos9_vectors.pl [seed [count]] > tests/data/junos9_vectors.json
#
# Every vector is a secret and the string Crypt::Juniper encrypted it to;
# `tests/junos9_interop.rs` checks that the crate decodes each of them.
# The output is deterministic for a given seed and count.

use strict;
use warnings;

use Crypt::Juniper;
use JSON::PP;

my $seed  = shift // 20261113;
my $count = shift // 600;
srand($seed);

my @plains = (
    'lc', 'a', 'juniper123', 'Juniper', 'netscreen', 'P@ssw0rd!', 'correct horse battery staple',
    '!"#$%&\'()*+,-./:;<=>?@[\\]^_`{|}~', ' leading and trailing ', '0123456789',
    'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz',
);
for (1 .. $count) {
    my $length = 1 + int rand 64;
    push @plains, join '', map { chr(0x20 + int rand 95) } 1 .. $length;
}

my @vectors;
for my $plain (@plains) {
    my $encoded = juniper_encrypt($plain);
    juniper_decrypt($encoded) eq $plain or die "Crypt::Juniper does not round-trip '$plain'";
    push @vectors, { plain => $plain, encoded => $encoded };
}

print JSON::PP->new->canonical->encode({
    meta    => { generator => 'tests/generate_junos9_vectors.pl', module => "Crypt::Juniper $Crypt::Juniper::VERSION", seed => $seed + 0, count => $count + 0 },
    vectors => \@vectors,
}), "\n";
