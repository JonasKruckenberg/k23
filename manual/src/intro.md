# k23 - Experimental WASM Operating System

Welcome to the official **k23 manual**! This manual will guide you through the installation and usage of *k23*, an
experimental WASM microkernel operating system. [GitHub repo](https://github.com/JonasKruckenberg/k23)

<br />

**Watch my talk at RustNL 2024 about k23**

<iframe width="560" height="315" src="https://www.youtube-nocookie.com/embed/GjDwj7RWOgs?si=bKBI4WKpm1HQ8YtP" title="YouTube video player" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share" referrerpolicy="strict-origin-when-cross-origin" allowfullscreen></iframe>

## What is k23?

k23 is an active research project exploring a *more secure, modular, and easy to develop for* operating system by using
WebAssembly as the primary execution environment.
The project is still in its early stages and is not yet ready for production use.

## Why?

As the world has changed, so has the way we interact with computers. When UNIX was invented in the 1960s, the world was
a very different place.
Time-sharing, the concept of multiple users sharing a single computer, was the hot new thing and having a wold-spanning
connected system was a pipe dream. And while countless people have worked incredibly hard to adapt the old systems to
the new world, it is clear that the old systems are not up to the task.

In todays massively interconnected world, where security is paramount maybe, *just maybe*, there is an opportunity for a
new OS to rethink how we can build secure, scalable and understandable systems for the 21st century.

## How?

k23 is built around the idea of using WebAssembly as the primary execution environment. This allows for a number of
benefits:

- **Security**: WebAssembly is designed to run in a sandboxed environment, making it much harder to exploit.
- **Modularity**: WebAssembly modules can depend on each other, importing and exporting functionality and data, forming
  a modular system where dependency management is a **first class citizen**.
- **Portability**: WebAssembly is designed to be very portable. Forget questions like "is this binary compiled for amd64
  or arm?". k23 programs just run wherever.
- **Static Analysis**: WebAssembly is famous for being very easy to analyze. This means we can check for bad programs
  without even running them.

k23 also uses a microkernel architecture where only the most core kernel functionliaty and WASM runtime are running in
privileged mode. Everything else is implemented as a WebAssembly module, running in a strongly sandboxed environment.

### The JIT compiler

The core thesis of k23 is that **by directly integrating the compiler into the kernel, they enter into a symbiotic
relationship** where e.g. the kernels knowledge of the physical machine can inform specific optimization in the compiler
and the total knowledge of all programs running on the system by the compiler can inform various sppedups in the kernel.
Cool stuff that only becomes possible because os this is:

- **Zero-cost IPC calls.** By leveraging the total knowledge of all programs the kernel can reduce the cost of IPC calls
  to almost the cost of regular function calls.
- **Machine specific optimizations** The kernel knows the exacts capability of the machine, of each core and much more.
  Being tightly integrated allows for these details to feed into compiler optimization passes.
- **Program aware scheduling** The compiler collects information about each program such as instruction use, information
  about possibly hot loops etc. This information can be fed back into the scheduler to allow it to make more informed
  decisions, like using performance cores vs efficiency cores.

k23 uses [cranelift](https://cranelift.dev) as its JIT compiler backend.
