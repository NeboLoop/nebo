---
name: character-swap
description: How to swap the person in a video clip for a cast member (an AI character swap for an ad or a post), and how to add people to the cast. Use whenever the owner wants someone in a video replaced, a cast member added, or asks who is in the cast.
triggers:
  - character swap
  - swap the person
  - replace the person
  - replace the actor
  - swap cast
  - ai influencer
  - ai creator
---

# Character Swap

A swap puts a cast member in place of the person in a clip. The person
comes only from the cast, never from an image you pass. Nobody goes in a
video until the owner has confirmed them once.

A swap is only for replacing the person in a video that already exists.
The same character across new shots is not a swap: make each shot's start
frame of them as an image with their portrait in `references`, get the
owner's yes, and make each clip from its approved frame. A film's
characters (a project's `cast/`) are not this cast either.

## The cast

The cast is shared by every employee on this Nebo.

- See it: `generate_media` with kind "cast" and nothing else.
- Add someone: kind "cast", `cast` their name, `image` a hero photo:
  front-facing, full body, good light. The owner gets a one-time card to
  confirm consent (or that it is an AI persona, or themselves). If nobody
  was there to answer, call kind "cast" with just `cast` when the owner is
  in a chat with you, and the card shows again.
- More angles: the same call with another `image`; `hero: true` replaces
  the hero. For someone already confirmed, the owner confirms again on the
  card before a new image joins.
- If the owner says no, or the person is a public figure or a celebrity
  lookalike, stop. Don't look for another way.

## The recipe

1. Prepare the clip with the Nebo Media plugin (its video skill): 30
   seconds or less, 24 fps, 720p at most. Split a longer clip at its scene
   cuts and swap each piece.
2. `generate_media` with kind "video", `mode` "replace", `video` the
   prepared clip and `cast` the person's name. It can take up to half an
   hour; if it isn't done, call again with the `job` it names.
3. Put the original sound back and tag the file AI-generated: Nebo Media's
   `audio mix` with `audio-from` the prepared clip and `ai-generated`
   "true".
4. That result reaches the owner as a card by itself; don't share it again.

Never present a swapped person as a real customer's testimonial unless it
is one. Where the law requires it, the ad says it is AI-generated.
