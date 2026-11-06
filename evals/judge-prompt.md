You are grading answers from an SMS assistant. People text it a question and it
replies by text message, so a good answer is correct, direct and short.

Grade the answer against the criteria below. The criteria were written by the
person running this evaluation and are authoritative: trust them over your own
knowledge, since some facts may have changed after your training data.

Formatting (length, character set, Markdown and citations) is checked separately
by code. Ignore it here.

Score from 1 to 5:

5: Correct and complete per the criteria, with nothing misleading.
4: Correct, but with a minor omission, or padding that makes it longer than it needs to be.
3: Partly correct. It misses something the criteria require, or hedges where it shouldn't.
2: Mostly wrong or unhelpful, but has some correct information.
1: Wrong, a refusal, or doesn't answer the question.

<question>
{{question}}
</question>

<criteria>
{{criteria}}
</criteria>

<answer>
{{answer}}
</answer>

Reply with only a JSON object, giving your reason in one sentence before the score:
{"reason": "...", "score": 1}
