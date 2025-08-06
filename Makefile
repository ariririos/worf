main: main.c
# 	gcc -o main -lmpdclient -lsqlite3 -Wall -Werror -g main.c
	gcc -o main -lmpdclient -lsqlite3 -Wall -Werror -g -fsanitize=address -fno-omit-frame-pointer main.c