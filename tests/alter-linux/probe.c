static __thread int counter = 41;
int alter_probe(void) { return ++counter; }
