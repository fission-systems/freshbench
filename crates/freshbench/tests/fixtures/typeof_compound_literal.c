/* Excerpt of a preprocessed translation unit of JustVugg/lumabri (Apache-2.0),
   src/lumabri_machine.c, used as a tree-sitter error-recovery fixture. */

static void cpu_profile(LmbMachineProfile *profile) {
    FILE *file = fopen("/proc/cpuinfo", "r");
    char line[1024], flags[4096] = "";
    unsigned processors = 0;
    struct { int package, core; } seen[1024];
    unsigned nseen = 0;
    int package = -1, core = -1;
    if (file) {
        while (fgets(line, sizeof line, file)) {
            if (!strncmp(line, "processor", 9)) {
                if (package >= 0 && core >= 0 && nseen < 1024) {
                    unsigned i = 0;
                    for (; i < nseen; i++)
                        if (seen[i].package == package && seen[i].core == core) break;
                    if (i == nseen) seen[nseen++] = (typeof(*seen)){package, core};
                }
                processors++; package = core = -1;
            } else if (!strncmp(line, "physical id", 11)) {
                char *colon = strchr(line, ':'); if (colon) package = atoi(colon + 1);
            } else if (!strncmp(line, "core id", 7)) {
                char *colon = strchr(line, ':'); if (colon) core = atoi(colon + 1);
            } else if ((!strncmp(line, "model name", 10) ||
                        !strncmp(line, "Hardware", 8) ||
                        !strncmp(line, "Model", 5)) && !profile->cpu_model[0]) {


                char *colon = strchr(line, ':');
                if (colon) {
                    while (*++colon == ' ' || *colon == '\t') {}
                    colon[strcspn(colon, "\r\n")] = 0;
                    snprintf(profile->cpu_model, sizeof profile->cpu_model, "%s", colon);
                }
            } else if ((!strncmp(line, "flags", 5) || !strncmp(line, "Features", 8)) &&
                       !flags[0]) {
                char *colon = strchr(line, ':');
                if (colon) snprintf(flags, sizeof flags, " %s ", colon + 1);
            }
        }
        if (package >= 0 && core >= 0 && nseen < 1024) {
            unsigned i = 0;
            for (; i < nseen; i++)
                if (seen[i].package == package && seen[i].core == core) break;
            if (i == nseen) seen[nseen++] = (typeof(*seen)){package, core};
        }
        fclose(file);
    }
    long online = sysconf(_SC_NPROCESSORS_ONLN);
    profile->logical_cpus = online > 0 ? (uint32_t)online : processors;
    profile->physical_cores = nseen ? nseen : profile->logical_cpus;



    int affinity_known = 0;
    for (unsigned n = 128; n <= 65536; n *= 2) {
        size_t bytes = ((((n) + (8 * sizeof (__cpu_mask)) - 1) / (8 * sizeof (__cpu_mask))) * sizeof (__cpu_mask));
        cpu_set_t *set = __sched_cpualloc (n);
        if (!set) break;
        do __builtin_memset (set, '\0', bytes); while (0);
        int rc = sched_getaffinity(0, bytes, set), saved = (*__errno_location ());
        if (!rc) {
            unsigned count = (unsigned)__sched_cpucount (bytes, set);
            affinity_known = count != 0;
            if (count && count < profile->logical_cpus) profile->logical_cpus = count;
        }
        __sched_cpufree (set);
        if (!rc || saved != 22) break;
    }
    if (!affinity_known) profile->logical_cpus = 1;
    LmbContainerLimits budget;
    if (lmb_container_limits_probe(&budget)) profile->logical_cpus = 1;
    else if (profile->logical_cpus > budget.cpu_threads)
        profile->logical_cpus = budget.cpu_threads;
    if (profile->physical_cores > profile->logical_cpus)
        profile->physical_cores = profile->logical_cpus;

    const char *isa = "generic";
    if (strstr(flags, " avx512_bf16 ") || strstr(flags, " avx512bf16 ")) isa = "avx512bf16";
    else if (strstr(flags, " avx512_vnni ") || strstr(flags, " avx512vnni ")) isa = "avx512vnni";
    else if (strstr(flags, " avx512f ")) isa = "avx512f";
    else if (strstr(flags, " avx_vnni ") || strstr(flags, " avxvnni ")) isa = "avxvnni";
    else if (strstr(flags, " avx2 ")) isa = "avx2";
    else if (strstr(flags, " asimd ")) isa = "asimd";
    snprintf(profile->isa, sizeof profile->isa, "%s", isa);

}
static int after(int v) {
  return v + 1;
}
