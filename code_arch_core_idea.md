# Code Arch — Core Project Idea

> **Code Arch** is short for **Code Architecture**.

## Project Concept

Code Arch is a lightweight local tool that helps AI coding agents understand large codebases with far less context consumption.

The problem it targets is simple: when an AI coding agent starts working inside an unfamiliar repository, it often spends a large amount of time and tokens rediscovering the same basic information.

It has to figure out what the project does, where important features live, how different parts of the system connect, which files matter for a task, what the main execution flows are, and which files can safely be ignored.

On a small project, this is not a major issue.

On a large repository, however, this discovery process can become expensive and repetitive. The agent may inspect many files, search repeatedly across the repository, follow imports, read configuration files, and reconstruct the architecture before it can even begin the requested task.

Code Arch aims to reduce that discovery cost.

Its purpose is to analyze a codebase locally and create a compact description of the project that preserves the information most useful for navigation and understanding.

The result is a small agent-friendly context file that gives a coding agent a mental model of the repository before it starts exploring the source code.

---

## The Core Problem

Modern coding agents are powerful, but they still need context.

A typical request might be:

> Change how subscription cancellation works.

Before making the change, the agent may need to discover:

1. Where billing logic exists.
2. Which files communicate with Stripe.
3. Where subscriptions are stored.
4. Which API route initiates cancellation.
5. Which webhook updates the database afterward.
6. Which tests cover the behavior.
7. Whether other modules depend on the same logic.

The source code already contains this information, but it is scattered across many files.

An agent therefore has to reconstruct a working mental model of the repository.

That reconstruction consumes tokens and tool calls.

For example, an agent might start with:

```text
Search for "subscription"
Read billing service
Search for Stripe references
Read checkout handler
Read webhook
Search for database model
Read subscription repository
Search for cancellation tests
Read test files
```

Some of this exploration is necessary, but a large portion is simply orientation.

The same orientation work can happen again in a future conversation because the next agent session does not necessarily retain the repository knowledge discovered previously.

Code Arch tries to make that knowledge persistent.

---

## The Main Idea

The basic transformation is:

```text
Large source repository

        ↓

Local analysis

        ↓

Compact representation of the project's functionality,
structure, relationships, and important navigation paths

        ↓

CODEBASE.md

        ↓

Coding agent starts with useful project context
instead of rediscovering everything from scratch
```

The generated file is not intended to contain all source code.

It is also not intended to replace the repository.

It acts as a map.

The source code remains the full territory.

A useful analogy is that Code Arch gives the agent the table of contents, road map, and important landmarks before asking it to navigate a very large city.

---

## What the Generated Context Should Explain

The generated context should answer questions such as:

### What is this project?

A short explanation of the product or software system.

Example:

```text
This is a Next.js SaaS application that allows users to create accounts,
purchase subscriptions, manage billing, and access protected content.
```

### What technologies does it use?

Example:

```text
Frontend: Next.js and React
Database: PostgreSQL
ORM: Prisma
Authentication: Auth.js
Payments: Stripe
Email: Resend
```

### What are the major functional areas?

Example:

```text
Authentication
Billing
User management
Database
Email
Admin
Frontend
API
```

### Where does each important feature live?

Example:

```text
Authentication:
src/auth/
src/middleware.ts
src/api/login/

Billing:
src/services/billing/
src/api/checkout/
src/webhooks/stripe/
```

### How do important parts connect?

Example:

```text
Login request
→ authentication service
→ user repository
→ database
→ session creation
```

### What files should an agent inspect for common tasks?

Example:

```text
For billing changes, inspect:
billing_service.ts
stripe_client.ts
subscription_repository.ts
stripe_webhook.ts
```

### What can probably be ignored?

Example:

```text
Generated files
Build output
Vendored dependencies
Static assets
Migration snapshots
```

The purpose is not perfect documentation.

The purpose is efficient orientation.

---

## Why This Could Reduce Token Usage

Without Code Arch, a coding agent may have to read many files before identifying the relevant ones.

Imagine a repository containing:

```text
2,000 source files
400,000 lines of code
1,200,000 estimated source tokens
```

A user asks:

> Fix a problem with password reset emails.

The agent does not need all 1.2 million tokens.

It may only need information from:

```text
password_reset.ts
email_service.ts
user_repository.ts
token_service.ts
password_reset.test.ts
```

The problem is discovering those files.

Code Arch attempts to provide enough initial context for the agent to immediately understand:

```text
Password reset belongs to the Authentication domain.

The flow is:

PasswordResetRoute
→ TokenService
→ UserRepository
→ EmailService

Relevant files:
...
```

The agent can then open those specific files.

The intended effect is:

```text
Less repository searching
Less irrelevant file reading
Fewer exploratory tool calls
Lower token consumption
Faster task completion
```

---

## The Meaning of "Compression"

Code Arch does not try to compress source code in the traditional sense.

It compresses the knowledge required to navigate the source code.

This distinction is important.

A million-token repository cannot be perfectly represented in a few thousand tokens without losing information.

But a coding agent does not need every implementation detail before starting a task.

It mainly needs a high-level understanding of where information is located and how major components relate.

Therefore:

```text
Source code:
Full implementation detail

CODEBASE.md:
Compressed navigation and architecture knowledge
```

The generated file tells the agent where to look.

The actual files tell the agent exactly how the implementation works.

---

## Example

Imagine this repository:

```text
src/
    auth/
        login.ts
        session.ts
        password_reset.ts

    billing/
        checkout.ts
        subscription.ts
        stripe.ts

    users/
        user_service.ts
        user_repository.ts

    email/
        email_service.ts

    database/
        client.ts
```

A coding agent seeing the repository for the first time has to infer how everything relates.

Code Arch might generate:

```md
# Project Overview

Subscription-based web application.

# Main Domains

## Authentication

Handles login, session management, and password recovery.

Important files:
src/auth/login.ts
src/auth/session.ts
src/auth/password_reset.ts

Password reset flow:

PasswordResetRoute
→ UserRepository
→ ResetToken
→ EmailService

## Billing

Handles checkout and subscription lifecycle.

Important files:
src/billing/checkout.ts
src/billing/subscription.ts
src/billing/stripe.ts

Checkout flow:

CheckoutRoute
→ Stripe
→ SubscriptionService
→ UserRepository

## Users

Responsible for user persistence and account data.

Important files:
src/users/user_service.ts
src/users/user_repository.ts

# Task Navigation

For password reset changes:
Read authentication/password_reset.ts
Read email/email_service.ts
Read users/user_repository.ts

For subscription changes:
Read billing/subscription.ts
Read billing/stripe.ts
```

The agent has not yet seen the implementation.

But it already has a useful mental model.

---

## Local-First Design

A central part of the idea is that Code Arch runs locally.

Source code can contain:

```text
Private business logic
Credentials accidentally stored in code
Internal APIs
Proprietary algorithms
Customer-specific integrations
Unreleased features
```

A developer should not need to send the whole repository to another cloud service simply to generate a project map.

Code Arch should therefore perform its analysis on the user's own machine.

The intended experience is:

```text
Install Code Arch

Run:

codearch .

Receive:

CODEBASE.md
```

No account should be required.

No cloud API should be necessary.

The tool should remain usable offline once its required components are installed.

---

## The Real Core: Structural Extraction

The hardest part of Code Arch is not summarization. It is correctly decomposing arbitrary codebases into subsystems and relationships.

This distinction determines the entire architecture of the tool.

If the structural layer is accurate, a very small model can label it well.

If the structural layer is wrong, a large model will produce a confident but incorrect map, which is worse than no map at all, because the coding agent will trust it and be actively misdirected.

Therefore the engineering effort belongs in extraction, not in model scale.

---

## Three Tiers of Codebase Visibility

Repositories differ less by language than by how much of the architecture is actually visible in the source.

### Tier 1 — Explicit imports

```text
TypeScript, Go, Rust, modern Python
```

Static analysis yields a real dependency graph.

```text
import { UserRepository } from './users'
```

is ground truth. This is the easy tier and the one most examples assume.

### Tier 2 — Convention-driven frameworks

```text
Rails, Django, Laravel, Spring
```

Structure lives in naming conventions and framework autoloading.

A Rails controller may reference a model with no import statement anywhere in the file. The import graph is nearly empty and reveals little.

### Tier 3 — Indirection-heavy codebases

```text
Dependency injection
Event buses
Message queues
Plugin registries
Reflection and dynamic dispatch
```

Here the relationships are not in the code in any form a parser recognizes. A dependency may exist only in an XML config, an annotation, or a runtime container registration.

A tool that handles Tier 1 and silently fails on Tiers 2 and 3 does not handle "all codebases," and those tiers cover a large share of real enterprise repositories.

---

## The Extraction Stack

### Tree-sitter as the universal layer

Grammars exist for many languages behind a single API. It is fast and error-tolerant, parsing broken or partial files rather than failing, which matters when scanning an unknown repository.

Limitation: it provides syntax, not semantics. It reports that a symbol was referenced, not which definition it resolves to.

### Per-language resolvers

Resolving `./users` to an actual file requires ecosystem knowledge:

```text
tsconfig.json path mappings
Python package semantics
Go modules
Java classpaths
```

This does not generalize. It is where most engineering time will go. A small number of ecosystems should be supported properly rather than many supported shallowly.

### Git co-change as a language-agnostic signal

Files repeatedly modified in the same commit tend to belong to the same subsystem.

This requires no parser, behaves identically across languages, and captures exactly the relationships static analysis misses: DI wiring, event producers and consumers, config-and-implementation pairs.

It is noisy alone, but strong as a second signal used to validate or correct import-graph clustering. Cost is a single `git log` traversal.

### Framework detection as a prior

Read manifests first:

```text
package.json
Gemfile
pom.xml
pyproject.toml
```

If the project is Next.js, the tool already knows that `app/` routes map to URLs and that `middleware.ts` is authentication-adjacent.

Convention frameworks are simultaneously where import graphs fail worst and where hardcoded priors work best. That inversion is an opportunity rather than a problem.

---

## Grouping Is Structural, Not Semantic

Subsystem boundaries should be derived from the graph, not invented by the model.

```text
Community detection (Louvain or Leiden) on the dependency graph
+ directory structure
+ git co-change weighting
    ↓
candidate subsystems
    ↓
LLM names and describes them
```

A small code embedding model can supplement this by grouping files that clearly belong together but that the import graph does not connect.

The generative model never decides what a subsystem *is*. It only writes the label and the sentence.

---

## Scope and Graceful Degradation

Broad shallow coverage is worse than narrow reliable coverage.

The failure mode of a universal tool that degrades quietly is a confidently wrong architecture that the agent will trust.

The tool should therefore:

```text
Support a small number of ecosystems properly
State its confidence level per subsystem
Emit "structure could not be reliably determined"
    instead of guessing
```

This also draws a clean boundary for the model:

> Where structure is visible, the LLM only names what the graph found.
> Where structure is invisible, the tool reports uncertainty rather than asking the model to infer relationships from code semantics — the task small models fail at.

---

## Why Use a Small Local LLM?

Some parts of code understanding are easy to determine mechanically.

A tool can identify:

```text
Files
Functions
Classes
Imports
Dependencies
Routes
Exports
References
```

without using an LLM.

However, some questions are semantic.

For example:

```text
What is the responsibility of this group of files?

Would these files be described as authentication or user management?

What short explanation best describes this subsystem?

What is the likely purpose of this execution flow?
```

This is where a small local LLM can help.

The important point is that the model should not be asked to understand an entire giant repository.

Instead, Code Arch should first reduce the problem.

The model might receive:

```text
Files:
login.ts
session.ts
password_reset.ts

Functions:
authenticateUser
createSession
resetPassword
validateResetToken

Dependencies:
UserRepository
EmailService
JWT
```

and generate:

```text
Authentication subsystem responsible for login,
session lifecycle, and password recovery.
```

That is a much simpler task than giving the model tens of thousands of lines of raw code.

Because the task has already been simplified, the project can attempt to use a very small model.

---

## Small Model Requirement

One of the project's key constraints is that the semantic model should work on normal consumer hardware.

The goal is not:

> Use the strongest local model available.

The goal is:

> Use the smallest model that is good enough after the code has already been structurally simplified.

A reasonable target would be approximately:

```text
0.5B to 1.5B parameters
```

with a lightweight quantized version.

The project should ideally work on:

```text
8 GB RAM
CPU-only laptop
No dedicated GPU required
```

A GPU may make processing faster, but it should not be mandatory.

This constraint makes the project more interesting because the quality must come from how well the repository is reduced and structured, rather than from brute-force model size.

---

## Model Selection

### Default

```text
Qwen2.5-Coder-1.5B-Instruct
Q4_K_M GGUF via llama.cpp
~1 GB on disk
Apache 2.0
```

### Why this one

The model's real job is reading identifiers such as `authenticateUser`, `validateResetToken`, `UserRepository`, and `middleware.ts` and recognizing what they mean as a group. That is identifier semantics and framework-convention recognition, which comes from code pretraining rather than general reasoning.

```text
Code-pretrained on exactly the token distribution it receives
No reasoning trace — output tokens are the CPU bottleneck
Conventional architecture, mature GGUF support, no runtime risk
Permissive license suitable for a distributed CLI tool
```

### Why not the newer general small models

Current general-purpose small models (for example the Qwen3.5 0.8B–2B tier) are natively multimodal, reasoning-tuned, and carry very long context windows. All three are wasted or actively harmful here:

```text
Multimodal capacity — never used
Reasoning traces — verbose output on a 40-token task
262K context — inputs are a few hundred tokens by design
```

They remain viable with thinking mode disabled, and should be benchmarked, but they are not the natural fit.

### Candidates to benchmark against the default

```text
CodeGemma 2B              second code-native option
Gemma 4 E2B               Apache 2.0, native structured JSON output
Qwen3.5-2B                thinking mode disabled
Llama 3.2 1B              fastest on CPU, weakest quality
```

### Known open question

Code-specialized models are tuned to continue code, not to write short English descriptions of it. It is possible that a code model recognizes identifiers better but writes worse prose than a general model. This is testable and should be resolved by evaluation rather than assumption.

### Implementation requirements

```text
Grammar-constrained decoding (GBNF) to force valid JSON
    — format failure becomes impossible, leaving only quality to tune
Few-shot exemplars in every prompt
    — worth more than doubling parameters at this scale
Output capped at ~40 tokens per description
--model flag over GGUF so the model is swappable
```

### Longer-term direction

The task is narrow, repetitive, and fixed-format — an ideal fine-tuning target.

```text
Generate training pairs by running a large model
over structured summaries from open-source repositories
    ↓
LoRA fine-tune Qwen2.5-Coder-0.5B
    ↓
Likely beats any off-the-shelf 2B while being smaller and faster
```

This is the same argument as the rest of the project applied to the model itself: quality from task-specific reduction rather than scale.

### Second model component

A small code embedding model is needed alongside the generative model, used for clustering support rather than text generation. It is a separate and much cheaper component.

---

## The Role of the LLM

The local LLM should be a helper, not the foundation of correctness.

It should be used for tasks such as:

```text
Naming detected subsystems
Writing short descriptions
Summarizing execution flows
Producing concise human-readable explanations
```

Note that grouping related functionality is deliberately **not** on this list. Grouping is determined structurally, as described above. The model labels clusters that the graph already found.

It should not be trusted to invent structural facts.

For example, if Code Arch says:

```text
BillingService depends on StripeClient
```

that should ideally come from actual code relationships.

The LLM may explain what the relationship probably means, but it should not create dependencies that were never found.

This keeps the generated context grounded in the repository.

---

## Persistent Project Memory

Another important part of the idea is persistence.

Today, coding agents frequently rediscover the same repository knowledge.

One session determines:

```text
Authentication is implemented here.
Billing works like this.
This directory contains generated code.
That service writes to the database.
```

A future session may repeat much of the same work.

Code Arch creates a reusable artifact.

```text
CODEBASE.md
```

can be stored inside the repository.

Then different tools can use the same map:

```text
Codex
Claude Code
Cursor
Other coding agents
Human developers
```

Instead of each agent reconstructing the project independently, they all start with the same compressed project knowledge.

---

## Keeping the Context Small

The tool should not generate a massive documentation file.

If `CODEBASE.md` itself becomes 50,000 or 100,000 tokens, much of the original benefit disappears.

The generated context should therefore prioritize information.

The idea is:

```text
Important architecture
Important features
Important relationships
Important execution flows
Important files
Useful navigation hints
```

while leaving out:

```text
Low-value helper functions
Repeated implementation details
Generated files
Boilerplate
Unimportant constants
Minor UI components
```

The output should be designed specifically for efficient agent consumption rather than exhaustive human documentation.

---

## Large Repositories

For a relatively small repository, one `CODEBASE.md` may be enough.

For a very large project, a better approach could eventually be:

```text
CODEBASE.md
    ↓
small top-level map

Authentication context
Billing context
Database context
Frontend context
```

The agent reads the small map first.

If a task concerns billing, it can then load only the billing context.

The central principle stays the same:

> Start with the smallest useful amount of information and load deeper context only when necessary.

---

## Example Agent Workflow

Without Code Arch:

```text
User:
Change the authentication timeout.

Agent:
List repository files.
Search for "auth".
Read several auth-related files.
Search for "session".
Read session files.
Search configuration.
Inspect middleware.
Search tests.
Determine which implementation controls timeout.
Make change.
```

With Code Arch:

```text
User:
Change the authentication timeout.

Agent reads CODEBASE.md.

CODEBASE.md says:

Authentication:
Session lifetime is managed by SessionService.
Configuration comes from auth/config.ts.
Middleware validates the session.
Tests are under tests/auth/session.test.ts.

Agent opens those files directly.

Agent makes change.
```

The second workflow is the behavior Code Arch is trying to enable.

---

## Main Hypothesis

The project is based on one measurable hypothesis:

> A coding agent supplied with a compact and accurate repository map will consume less context and perform less exploratory work than the same agent working from the raw repository alone.

This should eventually be tested.

The project becomes much stronger if the result can be measured instead of assumed.

---

## How the Idea Could Be Tested

Choose an open-source repository and several tasks.

Examples:

```text
Explain how login works.

Find where subscription status is updated.

Modify session expiration.

Add validation to the registration endpoint.

Identify which components would be affected by changing the User model.
```

Run the tasks twice.

First:

```text
Agent + normal repository
```

Then:

```text
Agent + repository + CODEBASE.md
```

Measure:

```text
Input tokens
Files opened
Search calls
Tool calls
Time to completion
Correctness
```

A useful result might look like:

```text
Without Code Arch

61,000 input tokens
31 files opened
14 searches

With Code Arch

34,000 input tokens
15 files opened
5 searches
```

The exact numbers do not matter until they are measured.

What matters is whether the concept produces a real improvement.

---

## Evaluating Decomposition Correctness

Decomposition is subjective. Two competent engineers will draw subsystem boundaries differently on the same repository, so there is no clean ground truth to compare against.

The map should therefore not be evaluated directly.

It should be evaluated downstream:

> Does an agent given the map find the correct files faster and with fewer tokens?

This is objective, measurable, and sidesteps an unanswerable question about what the "right" architecture is.

A second, smaller evaluation set is also needed for model selection specifically:

```text
~20 hand-labeled subsystems from repositories already understood
    ↓
Swap a candidate model, re-run, compare
    ↓
Model choice becomes a ten-minute test
    rather than a reading exercise
```

The benchmark should cover all three visibility tiers, not only Tier 1 repositories.

---

## Why This Is Different From Simply Asking Codex to Summarize the Repository

A powerful coding agent could theoretically inspect an entire repository and create a similar description.

But doing that defeats part of the purpose.

The user would still need to pay the context cost of having the large agent explore the repository.

For a massive repository, that exploration may consume a significant number of tokens.

Code Arch moves the repetitive discovery work to a cheap local process.

The intended model is:

```text
Use lightweight local computation
for repository understanding.

Use expensive powerful coding agents
for the actual difficult coding task.
```

This creates a division of labor.

Code Arch provides orientation.

Codex or another strong agent performs reasoning and implementation.

---

## Why Not Just Use a Huge Local LLM?

A large local model could potentially understand more code directly, but it would undermine the project's constraints.

Large models require:

```text
More memory
More storage
More powerful GPUs
Longer inference time
More energy
```

The interesting challenge is whether code can be transformed into a representation that makes a tiny model sufficient.

The project therefore intentionally avoids solving the problem through model scale.

---

## User Experience

The project should feel extremely simple.

Example:

```bash
$ codearch .
```

Output:

```text
Analyzing repository...

Source files: 1,284
Estimated source context: 820,000 tokens

Generated:
CODEBASE.md

Generated context:
6,400 tokens
```

The developer can then tell a coding agent:

```text
Read CODEBASE.md before working on this project.
```

That is the core experience.

Everything else is secondary.

---

## The Core Value Proposition

For the developer:

> Stop paying an AI agent to repeatedly rediscover the architecture of your repository.

For the coding agent:

> Get a compact mental model of the codebase before reading implementation details.

For the project itself:

> Turn a large repository into a small amount of useful navigational context.

---

## What Would Make the Project Successful

Code Arch does not need to understand every detail of every repository.

It does not need to replace existing coding agents.

It does not need perfect architectural reconstruction.

It succeeds if it consistently helps an agent answer:

```text
Where should I look?

What parts matter?

How is this feature connected?

What files probably need to be inspected?
```

with substantially less exploration.

A simple implementation that reliably saves context is more valuable than a highly complex system that attempts to perfectly understand an entire codebase.

---

## What Makes the Idea Interesting

The interesting part is not simply that an LLM summarizes code.

Many tools can summarize code.

The interesting idea is the combination of:

```text
Local execution

Very small model

Structural decomposition across dissimilar codebases

Aggressive context reduction

Persistent project memory

Agent-focused output

Measurable token savings
```

The genuinely hard problem is the third item. Reorganizing arbitrary codebases correctly — including those where the architecture is not visible in the imports — is the part that decides whether the tool works.

The project effectively acts as a lightweight context preparation layer between a repository and a powerful coding agent.

---

## Short Definition

> Code Arch is a local tool that analyzes a software repository and generates a compact, persistent map of its functionality and important relationships so AI coding agents can understand the project with fewer tokens and less exploratory work.

---

## One-Sentence Version

> Analyze the repository locally once, compress its important architectural knowledge into a small agent-readable context file, and let coding agents spend their tokens solving the task instead of rediscovering the codebase.
