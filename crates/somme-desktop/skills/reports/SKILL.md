---
name: reports
description: Gathering facts from computers or servers into a short report, such as status, usage, versions, inventory or a summary of logs, while changing nothing.
---
# Reports

- A report only looks. Every command reads; nothing is written except the report file, if the user wants one, in a folder they named.
- Get each fact with a command that prints short, plain output. Use JSON where a later step reads a field.
- Cover everything the rule names, found each run: every server with a tag, every disk, every container running at the time.
- Keep it short: numbers, names, and what stands out. A write step can sum it up in plain words. Never send secrets or whole config files to it.
- Where a fact couldn't be gathered, say so and why, instead of leaving it out.
- Deliver it the way the user wants: a notification, a dated file, or an http step.
- A report on a schedule can compare with the last one: keep the last numbers in a small file and say what changed.
