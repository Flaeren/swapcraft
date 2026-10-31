# Security policy

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Use GitHub's private vulnerability reporting for this repository (the *Security* tab, *Report a vulnerability*). Include what you found, how to reproduce it and, if you can, a sample file.

You can expect an acknowledgement within a reasonable time. This is a small volunteer project, so there is no guaranteed response schedule.

## What matters most here

voidflare opens files you give it, with engines that parse complex formats. The cases that count are a crafted file that makes a converter run code, write outside its output folder or read files it was not given. Problems in an engine itself (WordCraft, GridCraft, DeckCraft, PdfCraft, CADCraft) are best reported to that project as well; we will pass fixes on in the next release.

The converter apps do not use the network. The hub asks GitHub for the latest release when it starts, so it can tell you a newer version exists (switch it off with AUTO-CHECK in the hub or the `VOIDFLARE_NO_UPDATE_CHECK` environment variable); the request carries no file and no file names. Anything else that makes them contact the internet without asking is a bug.
