# k23 Loader

The entry point and loader stage for k23. This binary loads the kernel,
sets up the machine for the kernel, including initial virtual memory mappings.

Note this is currently *not* a full independent bootloader since the kernel
will be compressed and inlined into the loader.
This loader stage exists so that we have a clean separation between physical memory 
mode and virtual memory mode (the kernel only ever operates in virtual memory mode) 
and we mostly avoid nasty things such as invalid pointers or having to update stack frames.