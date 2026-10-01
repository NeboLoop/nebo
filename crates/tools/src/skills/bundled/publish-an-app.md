---
name: publish-an-app
description: Publish an app to the marketplace with the owner, in the conversation. Use when the owner taps Publish on an app, or asks to put an app on the marketplace, list it, share it, submit it for review, or change its listing.
triggers:
  - publish this app
  - publish the app
  - put it on the marketplace
  - submit it for review
  - list my app
  - the listing
---

# Publish an App

The owner wants this app on the marketplace. You draft the listing, take the
screenshots, show it, and change it as they talk. Only the owner sends it:
the submission asks them on a card, and only their tap submits.

This needs App Developer mode (Bot settings, Developer). Without it the tools
below are not offered; say so plainly if the owner asks.

## The Flow

1. **Look at the app first.** Take a screenshot to see it working:
   `app_screenshot()`. If it shows an error or a blank page, fix the app
   before you go on; a listing of a broken app is refused at review.
2. **Take 3 to 5 listing screenshots** with `for_listing: true`, each showing
   something different: the main screen at phone size (390x844), the same at
   desktop size (1280x800), and the screens that show what it does
   (`path: "index.html#/scores"`). Give each a short `label`. The first one is
   the card image, so make it the best one.
3. **Draft the listing:** `app_listing()`. It is built from the
   app's manifest and code: name, short description (one line on the card),
   long description ("What it does" on the listing page), category, version,
   visibility, permissions. Rewrite what reads poorly in the same call: pass
   `short_description` and `long_description` written for a person deciding
   whether to install it. Lead with what it does for them, in plain words.
4. **Show the owner the listing** and ask what to change. They may say
   "shorter description", "call it Kart Rush", "use the second screenshot
   first", "make it unlisted". Each change is another
   `app_listing(...)` with only what changed; screenshots
   are reordered or dropped by passing their file ids in the new order.
5. **When the owner says to send it**, call `app_submit()`.
   They confirm on a card. If they tap "Not yet", keep shaping it with them.
   Never submit because you think it is ready, and never in a run nobody is
   watching.
6. **After it is submitted**, the review's outcome is posted in this chat
   when it comes. If changes are requested, read the reviewer's notes, fix
   the app or the listing with the owner, bump the version if the app
   changed, and submit again.

## Visibility

- **Public**: listed on the marketplace for everyone once review passes.
- **Unlisted**: not browsable; anyone with its link or install code can
  install it.
- **Private**: only the owner can install it.

## What Can't Be Published From Here

An app that runs a program of its own beside its page (a sidecar) needs the
full publishing tools; the draft says so. Tell the owner plainly.

## Words

Say app, listing, marketplace, review, screenshots. The owner is the owner.
