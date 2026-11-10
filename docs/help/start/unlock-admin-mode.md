---
id: unlock-admin-mode
title: Unlock Admin mode
type: howto
area: safety
summary: Allow changes with Touch ID or your Mac password, then lock again when you're done.
keywords: [admin, unlock, touch id, fingerprint, padlock, requires admin, read-only, button disabled, button greyed out, button grayed out, greyed out, grayed out, cant change anything, wont let me, cant click, nothing happens]
verified: false
related: [admin-mode, settings-security, use-look-only-mode]
---

# Unlock Admin mode

To make a change in SparkBox, unlock Admin mode with the padlock in the top bar.

## Before you start

- Look-only mode must be off. If the padlock reads **Look only**, follow [Use look-only mode](help:use-look-only-mode) to turn it off first.
- With **Live** data, you need Touch ID or your Mac password. With **Demo** data, unlocking asks for nothing.
- It takes a few seconds.

## Steps

### Unlock

1. Find the padlock in the top bar, beside the **Demo** or **Live** label. It reads **Read-only**.
2. Click the padlock. With Live data, macOS asks you to confirm. Use Touch ID, or type your Mac password. With Demo data, this step asks for nothing.
3. Look at the padlock again. It now reads **Admin**, with a clock that counts down to the next lock, such as **Admin · 14:58**.

You can now make changes. The other ways to unlock do the same thing:

| Way in | How |
|---|---|
| Keyboard | Press <kbd>⇧⌘L</kbd> |
| Menu bar | Choose **Cluster › Unlock Admin Mode…** |
| Command palette | Press <kbd>⌘K</kbd>, type `admin`, then click **Unlock Admin mode…** under **Cluster** |
| A locked button | Click it. The **Unlock Admin mode?** dialog opens and says what the button needs. Click **Unlock…** |

### Lock again

1. When you're done, click the padlock. It reads **Read-only** again. You can also press <kbd>⇧⌘L</kbd> or choose **Cluster › Lock Admin Mode**.

If you forget, Admin mode locks by itself after 15 minutes without a change by default. To choose a time from 5 minutes to 24 hours, see **Lock after** in [Settings › Security & privacy](help:settings-security#lock-after).

## What you should see

While Admin mode is unlocked, the padlock is lit and reads **Admin · 14:58**, counting down. Each change you make restarts the clock.

A minute before it locks, a message appears: "Admin mode locks in 1 min". To stay unlocked, click **Keep unlocked**.

To see more detail, open [Security settings](open:settings?section=security). The **Right now** row under **Admin mode** shows one of these:

| Right now | What it means |
|---|---|
| **Unlocked · confirmed it's you** | You confirmed in the last 5 minutes, so changes on this Mac are allowed too |
| **Unlocked · nodes only** | Changes on your cluster are allowed. Changes on this Mac will ask for Touch ID or your Mac password |
| **Locked** | A change you ask for waits until you unlock. [What SparkBox still does while locked](help:admin-mode#what-sparkbox-still-does-while-locked) lists the work that goes on |

## Why it works

Unlocking opens a gate inside the app, so SparkBox can run commands that change things. The gate stays open while you keep using it, and it shuts after the idle time. Touch ID or your Mac password proves that it's you and not someone passing by your Mac. A Live unlock also covers changes on this Mac for 5 minutes, and a password or key you type asks every time. [Read-only, Admin mode and Look only](help:admin-mode) explains why.

## If something goes wrong

- **"Admin mode stays locked" appears, with "That wasn't the right password or fingerprint."** macOS didn't accept what you gave it. Try again.
- **"Admin mode stays locked" appears, with "This Mac can't confirm it's you".** This Mac has no login password or Touch ID set up. Set one in the Mac's **System Settings**, then try again.
- **"No answer from macOS; try again."** The macOS prompt was left open for 3 minutes. Click the padlock again.
- **A dialog named Look only appears.** Look-only mode is on, and nothing can unlock until it's off. Follow [Use look-only mode](help:use-look-only-mode).
- **"Admin mode locked" appears.** Admin mode locked by itself, and the message says why. The usual reason is "No changes for 15 minutes, so SparkBox is read-only again." Unlock it again.
- **"Live data is on: unlocking now asks for Touch ID or your Mac password."** You switched to Live, which locks Admin mode. Unlock it again.

[Admin, look-only and safety messages](help:errors-admin-and-safety) lists the rest.

## Next steps

- [Read-only, Admin mode and Look only](help:admin-mode)
- [Settings: Security & privacy](help:settings-security)
- [Find out what SparkBox did](help:find-what-sparkbox-did)
