These rules bind every change to the code.

Three are never broken:

1. **The tests are always green.** Every commit builds and passes its tests. A commit that breaks this is fixed before anything else is done.
2. **Every change can be explained in a few sentences.** A change that cannot is too large or too clever: split it or simplify it. Prefer the obvious construction to the elegant one. Name things for what they are, and rename them when that changes. Someone who knows the project can guess where any code lives.
3. **Every behavior has a test, written first.** Write a test that fails, then the code that makes it pass. Coverage measures this and is not its goal: never write a test only to raise the number.

Four are habits:

4. **Write reluctantly.** Source code is a liability, and progress is not measured in lines. Before adding code, ask whether deleting code, changing configuration, or using an existing component would do. A change that removes lines is progress.
5. **Learn by building.** Work oscillates between an idea and a run of it: the smallest code that tests the idea, then running it, then revising. When an approach proves wrong, go back to the last green commit rather than patch forward.
6. **Not yet.** Optimization waits for measured slowness, scalability for real load, refactoring for the moment the next real change is hard to make or hard to explain.
7. **Handle problems when they appear.** One that breaks a rule above is fixed now. Any other is a decision about scope, made deliberately and never in passing.
