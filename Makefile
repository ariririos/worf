main: main.c
# 	gcc -o worf -lmpdclient -lsqlite3 -lm -lffcall -Wall -Werror -g main.c deps/dotenv-c/dotenv.c
	gcc -o worf -lmpdclient -lsqlite3 -lm -lffcall -Wall -Werror -g -fsanitize=address -fno-omit-frame-pointer main.c deps/dotenv-c/dotenv.c